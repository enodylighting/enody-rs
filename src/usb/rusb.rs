use std::{
    fmt::Debug,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use rusb::{Context, Device, DeviceDescriptor, DeviceHandle, UsbContext};
use tokio::{
    sync::{broadcast, mpsc, RwLock},
    task::JoinHandle,
};

use crate::{
    message::Message,
    runtime::remote::RemoteRuntimeConnection,
    serialization,
    usb::{
        UsbBackend, UsbDevice, UsbDeviceEvent, UsbIdentifier, ALL_IDENTIFIERS, EP01,
        MESSAGE_CHANNEL_SIZE, USB_BUFFER_SIZE,
    },
    Identifier,
};

pub(crate) const RUSB_LOG_LEVEL: rusb::LogLevel = rusb::LogLevel::None;

// currently only for EP01, will need to revisit when 2nd device is added.
const WRITE_ENDPOINT: u8 = 0x01;
const READ_ENDPOINT: u8 = 0x81;

const HOTPLUG_TASK_TIMEOUT: Duration = Duration::from_millis(1_000);

impl From<rusb::Error> for crate::Error {
    fn from(e: rusb::Error) -> Self {
        crate::Error::USB(e.to_string())
    }
}

pub(crate) fn rusb_log_shim(level: rusb::LogLevel, message: String) {
    match level {
        rusb::LogLevel::Error => log::error!("{}", message),
        rusb::LogLevel::Warning => log::warn!("{}", message),
        rusb::LogLevel::Info => log::info!("{}", message),
        rusb::LogLevel::Debug => log::debug!("{}", message),
        rusb::LogLevel::None => (),
    }
}

#[derive(Debug)]
struct RusbDeviceConnection {
    handle: Arc<DeviceHandle<Context>>,
    message_rx_task: JoinHandle<Result<(), crate::Error>>,
    shutdown: broadcast::Sender<()>,
}

impl RusbDeviceConnection {
    /// CDC ACM control interface.
    const CDC_CONTROL_INTERFACE: u8 = 0;
    /// CDC ACM data interface.
    const CDC_DATA_INTERFACE: u8 = 1;

    fn initialize_device_handle(
        device: &Device<Context>,
    ) -> Result<DeviceHandle<Context>, crate::Error> {
        let device_handle = device.open().map_err(|e| {
            log::warn!(
                "Failed to open device handle (bus={} addr={}): {:?}",
                device.bus_number(),
                device.address(),
                e
            );
            crate::Error::USB(e.to_string())
        })?;

        // Detatch any active kernel drivers for either interface.
        for interface in [Self::CDC_CONTROL_INTERFACE, Self::CDC_DATA_INTERFACE] {
            log::trace!("Checking kernel driver on interface {}", interface);
            match device_handle.kernel_driver_active(interface) {
                Ok(true) => match device_handle.detach_kernel_driver(interface) {
                    Ok(_) => log::trace!("Detached kernel driver from interface {}", interface),
                    Err(e) => log::trace!(
                        "Could not detach kernel driver from interface {}: {:?}",
                        interface,
                        e
                    ),
                },
                Ok(false) => log::trace!("No kernel driver active on interface {}", interface),
                Err(e) => log::trace!(
                    "Could not query kernel driver status on interface {}: {:?}",
                    interface,
                    e
                ),
            }
        }

        // attempt to claim the CDC data interface. This may fail on Windows depening
        // on what driver is currently active.
        log::trace!("Claiming USB interface {}", Self::CDC_DATA_INTERFACE);
        device_handle
            .claim_interface(Self::CDC_DATA_INTERFACE)
            .inspect_err(|e| {
                log::warn!(
                    "Failed to claim USB interface {}: {:?}",
                    Self::CDC_DATA_INTERFACE,
                    e
                );
            })?;
        Ok(device_handle)
    }

    fn open(device: &Device<Context>) -> Result<(Self, mpsc::Receiver<Message>), crate::Error> {
        log::debug!(
            "Opening USB device: bus={} address={} (VID={:#06x} PID={:#06x})",
            device.bus_number(),
            device.address(),
            device.device_descriptor().unwrap().vendor_id(),
            device.device_descriptor().unwrap().product_id()
        );

        // initialize the connection handle to device
        let device_handle = Self::initialize_device_handle(device)?;
        let shared_device_handle = Arc::new(device_handle);

        // initialize message task communication queues
        let (message_tx, message_rx) = mpsc::channel(MESSAGE_CHANNEL_SIZE);
        let (shutdown, _) = broadcast::channel(1);
        let task_shutdown = shutdown.subscribe();

        // initialize message receive task. rusb does not offer async API, so
        // a sync API is made async via an independent task polling
        let message_rx_closure =
            Self::message_rx_closure(&shared_device_handle, message_tx, task_shutdown);
        let message_rx_task = tokio::task::spawn_blocking(message_rx_closure);

        let instance = Self {
            handle: shared_device_handle,
            message_rx_task,
            shutdown,
        };
        Ok((instance, message_rx))
    }

    async fn close(self) -> Result<(), crate::Error> {
        // stop the message rx task
        let _ = self.shutdown.send(());
        let task_result = self
            .message_rx_task
            .await
            .map_err(|_| crate::Error::USB("failed to join message_rx_task".to_string()))?;
        task_result?;

        // release the handle
        if let Err(e) = self.handle.release_interface(Self::CDC_DATA_INTERFACE) {
            log::warn!("Failed to release USB interface: {:?}", e);
        }

        Ok(())
    }

    fn message_rx_closure(
        device_handle: &Arc<DeviceHandle<Context>>,
        message_tx: mpsc::Sender<Message>,
        mut task_shutdown: broadcast::Receiver<()>,
    ) -> impl FnOnce() -> Result<(), crate::Error> {
        let task_handle = device_handle.clone();
        move || {
            let mut message_buffer = Vec::<u8>::new();
            let mut escaped = false;
            let timeout = Duration::from_millis(100);
            loop {
                let mut read_buffer = [u8::default(); USB_BUFFER_SIZE];
                let read_result = task_handle
                    .read_bulk(READ_ENDPOINT, &mut read_buffer, timeout)
                    .map_err(|e| crate::Error::USB(e.to_string()));

                if let Ok(bytes_read) = read_result {
                    for byte in read_buffer.iter().take(bytes_read) {
                        if message_buffer.is_empty() && *byte != serialization::CONTROL_CHAR_STX {
                            continue;
                        }

                        message_buffer.push(*byte);

                        if *byte == serialization::CONTROL_CHAR_ETX && !escaped {
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

                        escaped = (*byte == serialization::CONTROL_CHAR_DLE) && !escaped;
                    }
                }

                if let Ok(_reason) = task_shutdown.try_recv() {
                    log::trace!("USB connection read task shutdown");
                    return Ok(());
                }
            }
        }
    }

    fn handle(&self) -> &Arc<DeviceHandle<Context>> {
        &self.handle
    }

    fn serial_number(&self) -> Option<String> {
        let descriptor = self.handle.device().device_descriptor().ok()?;
        self.handle
            .read_serial_number_string_ascii(&descriptor)
            .ok()
    }
}

#[derive(Debug)]
struct RusbDevice {
    connection: RwLock<Option<RusbDeviceConnection>>,
    device: Device<Context>,
    identifier: Identifier,
    message_rx: Mutex<Option<mpsc::Receiver<Message>>>,
}

impl RusbDevice {
    fn new(device: Device<Context>) -> Self {
        let identifier = Identifier::new_v4();
        Self {
            connection: RwLock::new(None),
            device,
            identifier,
            message_rx: Mutex::new(None),
        }
    }
}

#[async_trait]
impl RemoteRuntimeConnection for RusbDevice {
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
        let message_rx = {
            let mut connection = self.connection.write().await;

            // check if connection was previously established
            if connection.is_some() {
                return Ok(());
            }

            let (opened_connection, message_rx) = RusbDeviceConnection::open(&self.device)?;
            *connection = Some(opened_connection);
            message_rx
        };

        let mut rx = self.message_rx.lock().unwrap();
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

        let mut rx = self.message_rx.lock().unwrap();
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
        connection
            .handle()
            .write_bulk(WRITE_ENDPOINT, &payload, Duration::from_millis(1000))?;

        Ok(())
    }

    async fn recv_message(&self) -> Result<Message, crate::Error> {
        let rx = {
            let mut rx_guard = self.message_rx.lock().unwrap();
            rx_guard.take()
        };
        let Some(mut rx) = rx else {
            return Err(crate::Error::USB("No active connection".to_string()));
        };
        let received = rx
            .recv()
            .await
            .ok_or(crate::Error::USB("failed to recv message".to_string()));
        {
            let mut rx_guard = self.message_rx.lock().unwrap();
            *rx_guard = Some(rx);
        }
        received
    }
}

impl UsbDevice for RusbDevice {
    fn identifier(&self) -> UsbIdentifier {
        EP01
    }

    fn serial_number(&self) -> Option<String> {
        self.connection.try_read().ok()?.as_ref()?.serial_number()
    }

    fn connection_key(&self) -> String {
        let (vendor_id, product_id) = self
            .device
            .device_descriptor()
            .map(|descriptor| (descriptor.vendor_id(), descriptor.product_id()))
            .unwrap_or((0, 0));

        format!(
            "rusb:{}:{}:{}:{}",
            vendor_id,
            product_id,
            self.device.bus_number(),
            self.device.address()
        )
    }
}

#[derive(Debug)]
pub(crate) struct RusbBackend {
    context: Context,
    event_tx: mpsc::Sender<UsbDeviceEvent>,
    hotplug_monitor_task: Option<JoinHandle<Result<(), crate::Error>>>,
    hotplug_registrations: Vec<rusb::Registration<Context>>,
    task_shutdown: broadcast::Sender<()>,
}

impl UsbBackend for RusbBackend {
    fn init(event_tx: mpsc::Sender<UsbDeviceEvent>) -> Result<Self, crate::Error> {
        let mut context = Context::new()?;
        context.set_log_level(RUSB_LOG_LEVEL);
        context.set_log_callback(Box::new(rusb_log_shim), rusb::LogCallbackMode::Context);
        let (task_shutdown, _) = broadcast::channel(1);
        let instance = Self {
            context,
            event_tx,
            hotplug_monitor_task: None,
            hotplug_registrations: Vec::new(),
            task_shutdown,
        };
        Ok(instance)
    }

    fn attached(&self) -> Result<Vec<Box<dyn UsbDevice>>, crate::Error> {
        let devices = self.context.devices()?;
        log::debug!("Enumerating USB devices ({} total on bus)", devices.len());

        let mut attached_devices: Vec<Box<dyn UsbDevice>> = Vec::new();
        for device in devices.iter() {
            if let Ok(descriptor) = device.device_descriptor() {
                let descriptor_identifier = UsbIdentifier::from(descriptor);
                // Check if this device matches any identifier in ALL_IDENTIFIERS
                for identifier in ALL_IDENTIFIERS.iter() {
                    if &descriptor_identifier == identifier {
                        log::debug!(
                            "Found matching device: {} (VID={:#06x} PID={:#06x}) at bus={} addr={}",
                            identifier.name,
                            identifier.vendor_id,
                            identifier.product_id,
                            device.bus_number(),
                            device.address()
                        );
                        let rusb_device = RusbDevice::new(device);
                        attached_devices.push(Box::new(rusb_device));
                        break;
                    }
                }
            }
        }

        log::debug!("Found {} matching device(s)", attached_devices.len());
        Ok(attached_devices)
    }

    fn start_discovery(&mut self) -> Result<(), crate::Error> {
        if self.hotplug_monitor_task.is_some() {
            return Ok(());
        }

        // register all possible vendor id / product id pairings with rusb
        for usb_identifier in ALL_IDENTIFIERS {
            let handler = USBHotplugHandler::new(usb_identifier, self.event_tx.clone());
            let registration = rusb::HotplugBuilder::new()
                .vendor_id(usb_identifier.vendor_id)
                .product_id(usb_identifier.product_id)
                .enumerate(true)
                .register(self.context.clone(), Box::new(handler))?;
            self.hotplug_registrations.push(registration);
        }

        // us a tokio task to keep pressure on the rusb event handling API
        let task_usb_context = self.context.clone();
        let mut task_shutdown_rx = self.task_shutdown.subscribe();
        let hotplug_monitor_task = tokio::task::spawn_blocking(move || loop {
            task_usb_context.handle_events(Some(HOTPLUG_TASK_TIMEOUT))?;
            if task_shutdown_rx.try_recv().is_ok() {
                log::trace!("rusb hotplug monitor task shutdown");
                return Ok(());
            }
        });
        self.hotplug_monitor_task = Some(hotplug_monitor_task);

        Ok(())
    }

    fn stop_discovery(&mut self) -> Result<(), crate::Error> {
        // determine if discovery was already running
        if self.hotplug_monitor_task.is_none() {
            return Ok(());
        }

        // issue shutdown command and wait for completion
        if let Some(task) = self.hotplug_monitor_task.take() {
            let _ = self.task_shutdown.send(());
            let deadline = std::time::Instant::now() + HOTPLUG_TASK_TIMEOUT;
            while !task.is_finished() {
                if std::time::Instant::now() >= deadline {
                    task.abort();
                    return Err(crate::Error::Debug(
                        "rusb hotplug monitor task did not stop within timeout".to_string(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
struct USBHotplugHandler {
    usb_identifier: UsbIdentifier,
    event_tx: mpsc::Sender<UsbDeviceEvent>,
}

impl USBHotplugHandler {
    fn new(usb_identifier: UsbIdentifier, event_tx: mpsc::Sender<UsbDeviceEvent>) -> Self {
        Self {
            usb_identifier,
            event_tx,
        }
    }
}

impl rusb::Hotplug<Context> for USBHotplugHandler {
    fn device_arrived(&mut self, arrived_device: Device<Context>) {
        log::trace!("{} arrived", self.usb_identifier.name);
        let event = UsbDeviceEvent::Connected(Box::new(RusbDevice::new(arrived_device)));
        if let Err(e) = self.event_tx.try_send(event) {
            log::warn!("Failed to send arrived rusb event: {:?}", e);
        }
    }

    fn device_left(&mut self, left_device: Device<Context>) {
        log::trace!("{} left", self.usb_identifier.name);
        let event = UsbDeviceEvent::Disconnected(Box::new(RusbDevice::new(left_device)));
        if let Err(e) = self.event_tx.try_send(event) {
            log::warn!("Failed to send departed rusb event: {:?}", e);
        }
    }
}

impl From<DeviceDescriptor> for UsbIdentifier {
    fn from(descriptor: DeviceDescriptor) -> Self {
        UsbIdentifier {
            name: "Unknown",
            vendor_id: descriptor.vendor_id(),
            product_id: descriptor.product_id(),
        }
    }
}
