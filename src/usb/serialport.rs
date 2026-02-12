use std::{
    fmt::Debug,
    io::{ErrorKind, Read as _, Write as _},
    sync::mpsc as std_mpsc,
    thread::sleep,
    time::Duration,
};

use async_trait::async_trait;
use tokio::{
    sync::{broadcast, mpsc, Mutex as AsyncMutex, RwLock},
    task::JoinHandle,
};

use serialport::{SerialPort, SerialPortType};

use crate::{
    message::Message,
    runtime::remote::RemoteRuntimeConnection,
    usb::{UsbBackend, UsbDevice, UsbDeviceEvent, UsbIdentifier},
};
use crate::{
    serialization,
    usb::{ALL_IDENTIFIERS, MESSAGE_CHANNEL_SIZE, USB_BUFFER_SIZE},
};

impl From<serialport::Error> for crate::Error {
    fn from(e: serialport::Error) -> Self {
        let info = format!("serialport error: {:?}", e);
        crate::Error::USB(info.to_string())
    }
}

#[derive(Debug)]
struct SerialPortConnection {
    serial_tx: std_mpsc::Sender<Vec<u8>>,
    message_rx_task: JoinHandle<Result<(), crate::Error>>,
    shutdown: broadcast::Sender<()>,
}

impl SerialPortConnection {
    fn open_port(port_name: &str) -> Result<Box<dyn SerialPort>, crate::Error> {
        log::debug!("Opening serial device {}", port_name);

        let mut port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(1))
            .open()
            .map_err(|e| crate::Error::USB(e.to_string()))?;

        // Reduce latency and avoid toggling reset lines on open/close.
        let _ = port.set_timeout(Duration::from_millis(1));
        let _ = port.set_flow_control(serialport::FlowControl::None);
        let _ = port.set_data_bits(serialport::DataBits::Eight);
        let _ = port.set_parity(serialport::Parity::None);
        let _ = port.set_stop_bits(serialport::StopBits::One);

        port.write_data_terminal_ready(false)?;
        port.write_request_to_send(false)?;
        sleep(Duration::from_millis(100));

        // on windows the ESP32-C6 must be reboot upon opening a serial connection.
        // the usbser.sys driver asserts both lines immediately upon device attach
        // causing EP01 to boot into download mode.
        #[cfg(target_os = "windows")]
        {
            port.write_request_to_send(true)?;
            port.write_data_terminal_ready(false)?;
            port.write_request_to_send(true)?;

            sleep(Duration::from_millis(100));

            port.write_request_to_send(false)?;

            sleep(Duration::from_millis(100));
        }

        Ok(port)
    }

    fn message_rx_closure<InternalCommand, InternalEvent>(
        mut serial_port: Box<dyn SerialPort>,
        message_tx: mpsc::Sender<Message<InternalCommand, InternalEvent>>,
        serial_rx: std_mpsc::Receiver<Vec<u8>>,
        mut task_shutdown: broadcast::Receiver<()>,
    ) -> impl FnOnce() -> Result<(), crate::Error>
    where
        InternalCommand:
            Debug + Send + Sync + 'static + serde::de::DeserializeOwned + serde::Serialize,
        InternalEvent:
            Debug + Send + Sync + 'static + serde::de::DeserializeOwned + serde::Serialize,
    {
        move || {
            let mut message_buffer = Vec::<u8>::new();
            let mut escaped = false;
            let mut read_buffer = [0u8; USB_BUFFER_SIZE];

            loop {
                while let Ok(payload) = serial_rx.try_recv() {
                    if let Err(e) = serial_port.write_all(&payload) {
                        log::warn!("serial write error: {:?}", e);
                        return Err(crate::Error::USB(e.to_string()));
                    }
                    if let Err(e) = serial_port.flush() {
                        log::warn!("serial flush error: {:?}", e);
                        return Err(crate::Error::USB(e.to_string()));
                    }
                }

                match serial_port.read(&mut read_buffer) {
                    Ok(bytes_read) => {
                        for &byte in &read_buffer[..bytes_read] {
                            if message_buffer.is_empty() && byte != serialization::CONTROL_CHAR_STX
                            {
                                continue;
                            }

                            message_buffer.push(byte);

                            if byte == serialization::CONTROL_CHAR_ETX && !escaped {
                                match Message::<InternalCommand, InternalEvent>::try_from(
                                    message_buffer.clone(),
                                ) {
                                    Ok(message) => {
                                        log::trace!("received message: {:?}", message);
                                        if let Err(_e) = message_tx.try_send(message) {}
                                    }
                                    Err(e) => {
                                        log::trace!("failed to deserialize message: {:?}", e);
                                    }
                                }
                                message_buffer.clear();
                            }

                            escaped = byte == serialization::CONTROL_CHAR_DLE && !escaped;
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::TimedOut => {}
                    Err(e) => {
                        log::warn!("serial read error: {:?}", e);
                        return Err(crate::Error::USB(e.to_string()));
                    }
                }

                if let Ok(_reason) = task_shutdown.try_recv() {
                    log::trace!("Serial connection read task shutdown");
                    return Ok(());
                }
            }
        }
    }

    fn open<InternalCommand, InternalEvent>(
        port_name: &str,
    ) -> Result<
        (
            Self,
            mpsc::Receiver<Message<InternalCommand, InternalEvent>>,
        ),
        crate::Error,
    >
    where
        InternalCommand:
            Debug + Send + Sync + 'static + serde::de::DeserializeOwned + serde::Serialize,
        InternalEvent:
            Debug + Send + Sync + 'static + serde::de::DeserializeOwned + serde::Serialize,
    {
        let serial_port = Self::open_port(port_name)?;
        let (message_tx, message_rx) = mpsc::channel(MESSAGE_CHANNEL_SIZE);
        let (serial_tx, serial_rx) = std_mpsc::channel::<Vec<u8>>();
        let (shutdown, _) = broadcast::channel(1);
        let task_shutdown = shutdown.subscribe();

        let message_rx_closure =
            Self::message_rx_closure(serial_port, message_tx, serial_rx, task_shutdown);
        let message_rx_task = tokio::task::spawn_blocking(message_rx_closure);

        let connection = Self {
            serial_tx,
            message_rx_task,
            shutdown,
        };
        Ok((connection, message_rx))
    }

    async fn close(self) -> Result<(), crate::Error> {
        let _ = self.shutdown.send(());
        let task_result = self
            .message_rx_task
            .await
            .map_err(|_| crate::Error::USB("failed to join serial message task".to_string()))?;
        task_result
    }

    fn send_payload(&self, payload: Vec<u8>) -> Result<(), crate::Error> {
        self.serial_tx
            .send(payload)
            .map_err(|e| crate::Error::USB(e.to_string()))
    }
}

#[derive(Debug)]
pub(crate) struct SerialPortDevice<InternalCommand = (), InternalEvent = ()> {
    port_name: String,
    serial_number: Option<String>,
    connection: RwLock<Option<SerialPortConnection>>,
    message_rx: AsyncMutex<Option<mpsc::Receiver<Message<InternalCommand, InternalEvent>>>>,
}

impl<InternalCommand, InternalEvent> SerialPortDevice<InternalCommand, InternalEvent> {
    pub(crate) fn new(port_name: String, serial_number: Option<String>) -> Self {
        Self {
            port_name,
            serial_number,
            connection: RwLock::new(None),
            message_rx: AsyncMutex::new(None),
        }
    }
}

#[async_trait]
impl<InternalCommand, InternalEvent> RemoteRuntimeConnection<InternalCommand, InternalEvent>
    for SerialPortDevice<InternalCommand, InternalEvent>
where
    InternalCommand: Debug + Send + Sync + 'static + serde::de::DeserializeOwned + serde::Serialize,
    InternalEvent: Debug + Send + Sync + 'static + serde::de::DeserializeOwned + serde::Serialize,
{
    fn is_connected(&self) -> bool {
        self.connection
            .try_read()
            .map(|connection| connection.is_some())
            .unwrap_or(false)
    }

    async fn connect(&self) -> Result<(), crate::Error> {
        let mut connection = self.connection.write().await;
        if connection.is_some() {
            return Ok(());
        }

        let (opened_connection, message_rx) =
            SerialPortConnection::open::<InternalCommand, InternalEvent>(&self.port_name)?;
        *connection = Some(opened_connection);
        
        // free the lock before the next await
        drop(connection); 

        let mut rx = self.message_rx.lock().await;
        *rx = Some(message_rx);
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), crate::Error> {
        let connection = {
            let mut connection = self.connection.write().await;
            connection.take()
        };

        let Some(connection) = connection else {
            return Err(crate::Error::USB(
                "Attemped to disconnect from inexistent connection".to_string(),
            ));
        };
        connection.close().await?;

        let mut rx = self.message_rx.lock().await;
        *rx = None;
        Ok(())
    }

    async fn send_message(
        &self,
        message: Message<InternalCommand, InternalEvent>,
    ) -> Result<(), crate::Error> {
        let connection = self.connection.read().await;
        let Some(connection) = connection.as_ref() else {
            return Err(crate::Error::USB("No active connection".to_string()));
        };

        let payload: Vec<u8> =
            Vec::<u8>::try_from(message).map_err(|_| crate::Error::Serialization)?;
        connection.send_payload(payload)
    }

    async fn recv_message(&self) -> Result<Message<InternalCommand, InternalEvent>, crate::Error> {
        let mut rx = self.message_rx.lock().await;
        let Some(rx) = rx.as_mut() else {
            return Err(crate::Error::USB("No active connection".to_string()));
        };
        rx.recv()
            .await
            .ok_or(crate::Error::USB("failed to recv message".to_string()))
    }
}

impl<InternalCommand, InternalEvent> UsbDevice<InternalCommand, InternalEvent>
    for SerialPortDevice
{
    fn identifier(&self) -> UsbIdentifier {
        ALL_IDENTIFIERS[0]
    }

    fn serial_number(&self) -> Option<String> {
        self.serial_number.clone()
    }
}

#[derive(Default, Debug)]
pub(crate) struct SerialPortBackend {}

impl SerialPortBackend {
    pub(crate) fn attached_ports() -> Result<Vec<(String, serialport::UsbPortInfo)>, crate::Error> {
        let ports = serialport::available_ports().map_err(|e| crate::Error::USB(e.to_string()))?;
        let mut matching_ports = Vec::new();

        for port in ports {
            let SerialPortType::UsbPort(info) = port.port_type else {
                continue;
            };

            let product = info.product.as_deref();

            // validate the vendor id, product id, and on windows ensure the connection
            // is on "Interface 0". "Interface 2" will also be exposed, but that is the
            // JTAG.
            if ALL_IDENTIFIERS.iter().any(|identifier| {
                identifier.vendor_id == info.vid
                    && identifier.product_id == info.pid
                    && (cfg!(not(target_os = "windows"))
                        || product
                            .is_some_and(|name| name.contains("(Interface 0)")))
            }) {
                matching_ports.push((port.port_name, info));
            }
        }

        Ok(matching_ports)
    }
}

#[async_trait]
impl UsbBackend for SerialPortBackend {
    fn attached(&self) -> Result<Vec<Box<dyn UsbDevice>>, crate::Error> {
        let ports = Self::attached_ports()?;
        let mut devices: Vec<Box<dyn UsbDevice>> = Vec::new();

        for (port_name, port_info) in ports {
            let serial_number = port_info.serial_number;
            devices.push(Box::new(SerialPortDevice::new(port_name, serial_number)));
        }

        log::debug!("Found {} matching serial device(s)", devices.len());
        Ok(devices)
    }

    async fn next_device_event(&self) -> UsbDeviceEvent {
        std::future::pending().await
    }
}
