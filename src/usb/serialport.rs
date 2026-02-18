use std::{
    collections::HashMap,
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
    Identifier,
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

const HOTPLUG_TASK_TIMEOUT: Duration = Duration::from_millis(1_000);

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

    fn message_rx_closure(
        mut serial_port: Box<dyn SerialPort>,
        message_tx: mpsc::Sender<Message>,
        serial_rx: std_mpsc::Receiver<Vec<u8>>,
        mut task_shutdown: broadcast::Receiver<()>,
    ) -> impl FnOnce() -> Result<(), crate::Error> {
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
                                match Message::try_from(message_buffer.clone()) {
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

    fn open(port_name: &str) -> Result<(Self, mpsc::Receiver<Message>), crate::Error> {
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
pub(crate) struct SerialPortDevice {
    connection: RwLock<Option<SerialPortConnection>>,
    identifier: Identifier,
    message_rx: AsyncMutex<Option<mpsc::Receiver<Message>>>,
    port_name: String,
    serial_number: Option<String>,
}

impl SerialPortDevice {
    pub(crate) fn new(port_name: String, serial_number: Option<String>) -> Self {
        Self {
            connection: RwLock::new(None),
            message_rx: AsyncMutex::new(None),
            identifier: Identifier::new_v4(),
            port_name,
            serial_number,
        }
    }
}

#[async_trait]
impl RemoteRuntimeConnection for SerialPortDevice {
    fn identifier(&self) -> Identifier {
        self.identifier
    }

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

        let (opened_connection, message_rx) = SerialPortConnection::open(&self.port_name)?;
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

    async fn send_message(&self, message: Message) -> Result<(), crate::Error> {
        let connection = self.connection.read().await;
        let Some(connection) = connection.as_ref() else {
            return Err(crate::Error::USB("No active connection".to_string()));
        };

        let payload: Vec<u8> =
            Vec::<u8>::try_from(message).map_err(|_| crate::Error::Serialization)?;
        connection.send_payload(payload)
    }

    async fn recv_message(&self) -> Result<Message, crate::Error> {
        let mut rx = self.message_rx.lock().await;
        let Some(rx) = rx.as_mut() else {
            return Err(crate::Error::USB("No active connection".to_string()));
        };
        rx.recv()
            .await
            .ok_or(crate::Error::USB("failed to recv message".to_string()))
    }
}

impl UsbDevice for SerialPortDevice {
    fn identifier(&self) -> UsbIdentifier {
        ALL_IDENTIFIERS[0]
    }

    fn serial_number(&self) -> Option<String> {
        self.serial_number.clone()
    }

    fn connection_key(&self) -> String {
        format!(
            "serialport:{}:{}",
            self.port_name,
            self.serial_number.clone().unwrap_or_default()
        )
    }
}

#[derive(Debug)]
pub(crate) struct SerialPortBackend {
    event_tx: mpsc::Sender<UsbDeviceEvent>,
    poll_task: Option<JoinHandle<Result<(), crate::Error>>>,
    task_shutdown: broadcast::Sender<()>,
}

impl SerialPortBackend {
    pub(crate) fn attached_ports() -> Result<Vec<(String, serialport::UsbPortInfo)>, crate::Error> {
        let ports = serialport::available_ports().map_err(|e| crate::Error::USB(e.to_string()))?;
        let mut matching_ports = Vec::new();

        for port in ports {
            let SerialPortType::UsbPort(info) = port.port_type else {
                continue;
            };

            let product = info.product.as_deref().unwrap_or("");

            if ALL_IDENTIFIERS.iter().any(|identifier| {
                identifier.vendor_id == info.vid
                    && identifier.product_id == info.pid
                    && (cfg!(not(target_os = "windows")) || product.contains("(Interface 0)"))
            }) {
                matching_ports.push((port.port_name, info));
            }
        }

        Ok(matching_ports)
    }

    fn port_map(
        ports: Vec<(String, serialport::UsbPortInfo)>,
    ) -> HashMap<String, (String, Option<String>)> {
        let mut map = HashMap::new();
        for (port_name, port_info) in ports {
            let serial_number = port_info.serial_number;
            let key = format!(
                "serialport:{}:{}",
                port_name,
                serial_number.clone().unwrap_or_default()
            );
            map.insert(key, (port_name, serial_number));
        }
        map
    }
}

impl UsbBackend for SerialPortBackend {
    fn init(event_tx: mpsc::Sender<UsbDeviceEvent>) -> Result<Self, crate::Error> {
        let (task_shutdown, _) = broadcast::channel(1);
        let instance = Self {
            event_tx,
            poll_task: None,
            task_shutdown,
        };
        Ok(instance)
    }

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

    fn start_discovery(&mut self) -> Result<(), crate::Error> {
        if self.poll_task.is_some() {
            return Ok(());
        }

        let initial_ports = Self::attached_ports()?;
        let mut known_ports = Self::port_map(initial_ports);

        let event_tx = self.event_tx.clone();
        let mut task_shutdown_rx = self.task_shutdown.subscribe();

        let poll_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let current_ports = match SerialPortBackend::attached_ports() {
                            Ok(ports) => ports,
                            Err(e) => {
                                log::warn!("Failed to enumerate serial devices in discovery loop: {:?}", e);
                                continue;
                            }
                        };
                        let current_map = SerialPortBackend::port_map(current_ports);

                        for (key, (port_name, serial_number)) in current_map.iter() {
                            if known_ports.contains_key(key) {
                                continue;
                            }

                            let event = UsbDeviceEvent::Connected(Box::new(SerialPortDevice::new(port_name.clone(), serial_number.clone())));
                            if let Err(e) = event_tx.try_send(event) {
                                log::warn!("Failed to enqueue serial arrived event: {:?}", e);
                            }
                        }

                        for (key, (port_name, serial_number)) in known_ports.iter() {
                            if current_map.contains_key(key) {
                                continue;
                            }

                            let event = UsbDeviceEvent::Disconnected(Box::new(SerialPortDevice::new(port_name.clone(), serial_number.clone())));
                            if let Err(e) = event_tx.try_send(event) {
                                log::warn!("Failed to enqueue serial departed event: {:?}", e);
                            }
                        }

                        known_ports = current_map;
                    }
                    _ = task_shutdown_rx.recv() => {
                        log::trace!("serialport discovery polling task shutdown");
                        return Ok(());
                    }
                }
            }
        });
        self.poll_task = Some(poll_task);

        Ok(())
    }

    fn stop_discovery(&mut self) -> Result<(), crate::Error> {
        if self.poll_task.is_none() {
            return Ok(());
        }

        if let Some(task) = self.poll_task.take() {
            let _ = self.task_shutdown.send(());
            let deadline = std::time::Instant::now() + HOTPLUG_TASK_TIMEOUT;
            while !task.is_finished() {
                if std::time::Instant::now() >= deadline {
                    task.abort();
                    return Err(crate::Error::Debug(
                        "serial discovery polling task did not stop within timeout".to_string(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        Ok(())
    }
}
