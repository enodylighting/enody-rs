use std::{
    sync::Arc,
    time::Duration
};

use async_trait::async_trait;
use rusb::UsbContext;
use tokio::{
    sync::{
        Mutex as AsyncMutex,
        RwLock,
        broadcast,
        mpsc
    },
    task::JoinHandle
};

use crate::{
    Identifier,
    environment::{Environment, DiscoveryEnvironment},
    message::Message,
    runtime::remote::{RemoteRuntime, RemoteRuntimeConnection},
    serialization
};

const USB_BUFFER_SIZE: usize = 128;
const MESSAGE_CHANNEL_SIZE: usize = 64;
const RUSB_LOG_LEVEL: rusb::LogLevel = rusb::LogLevel::None;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct USBIdentifier {
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
}

const EP01: USBIdentifier = USBIdentifier {
    name: "EP01",
    vendor_id: 0x303A,
    product_id: 0x1001
};

const ALL_IDENTIFIERS: [USBIdentifier; 1] = [
    EP01
];

fn rusb_log_shim(level: rusb::LogLevel, message: String) {
    match level {
        rusb::LogLevel::Error => log::error!("{}", message),
        rusb::LogLevel::Warning => log::warn!("{}", message),
        rusb::LogLevel::Info => log::info!("{}", message),
        rusb::LogLevel::Debug => log::debug!("{}", message),
        rusb::LogLevel::None => (),
    }
}

/// Active connection state containing the USB handle and control channels.
struct ActiveConnection {
    handle: Arc<rusb::DeviceHandle<rusb::Context>>,
    message_rx_task: JoinHandle<Result<(), crate::Error>>,
    shutdown: broadcast::Sender<()>,
}

/// USB-based connection for communicating with Enody devices over USB.
///
/// This implements the `RemoteRuntimeConnection` trait as a naive message
/// transport, providing:
/// - USB device connection and disconnection
/// - Message serialization/deserialization with postcard
/// - Background task for continuous USB reading
///
/// This connection does NOT handle command-response matching - that logic
/// belongs in the RemoteRuntime layer.
///
/// All synchronization is handled internally, allowing this to be safely
/// shared via `Arc` without external locking.
pub struct USBConnection {
    device: USBDevice,
    active: RwLock<Option<ActiveConnection>>,
    /// Message receiver, separate from active connection to allow async recv without holding RwLock
    message_rx: AsyncMutex<Option<mpsc::Receiver<Message<(), ()>>>>,
}

impl std::fmt::Debug for USBConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("USBConnection")
            .field("device", &self.device)
            .field("connected", &self.active.try_read().map(|a| a.is_some()).unwrap_or(false))
            .finish_non_exhaustive()
    }
}

impl USBConnection {
    const USB_INTERFACE: u8 = 1;
    const WRITE_ENDPOINT: u8 = 0x01;
    const READ_ENDPOINT: u8 = 0x81;

    /// Create a new USBConnection for a device without connecting.
    /// Call `connect()` to establish the connection.
    pub fn new(device: USBDevice) -> Self {
        Self {
            device,
            active: RwLock::new(None),
            message_rx: AsyncMutex::new(None),
        }
    }

    /// Connect to all attached USB devices.
    pub async fn attached() -> Result<Vec<Self>, crate::Error> {
        let devices = USBDevice::attached()?;
        let mut connections = Vec::with_capacity(devices.len());
        for device in devices {
            let conn = Self::new(device);
            conn.connect().await?;
            connections.push(conn);
        }
        Ok(connections)
    }

    /// Initialize the USB device handle: open, detach kernel driver, claim interface.
    fn initialize_device_handle(&self) -> Result<rusb::DeviceHandle<rusb::Context>, crate::Error> {
        let device_handle = self.device.device.open()
            .map_err(|_| crate::Error::Debug("failed to open device handle".to_string()))?;

        // Detach kernel driver on Linux
        #[cfg(target_os = "linux")]
        if let Ok(active) = device_handle.kernel_driver_active(Self::USB_INTERFACE) {
            if active {
                match device_handle.detach_kernel_driver(Self::USB_INTERFACE) {
                    Ok(_) => log::info!("Detached kernel driver from interface {}", Self::USB_INTERFACE),
                    Err(e) => log::warn!("Could not detach kernel driver: {:?}", e),
                }
            }
        }

        // Claim interface for communication
        device_handle.claim_interface(Self::USB_INTERFACE)
            .map_err(|_| crate::Error::Debug("failed to claim USB interface".to_string()))?;

        // The usb stack on my development machine does a bunch of other shit when attaching
        // reset the ESP32-C6 to undo it all and get into a known state.
        // TODO(carter): Determine what setting is getting doinked to avoid reset.
        #[cfg(target_os = "linux")]
        {
            // Clear Download Flag
            // RTS = 0 DTR = 0
            let _ = device_handle.write_control(
                0x21,
                0x22,
                0x00,
                Self::USB_INTERFACE as u16,
                &[],
                Duration::from_millis(100),
            );

            // Reboot
            // RTS = 1 DTR = 0
            let _ = device_handle.write_control(
                0x21,
                0x22,
                0x02,
                Self::USB_INTERFACE as u16,
                &[],
                Duration::from_millis(100),
            );
        }

        Ok(device_handle)
    }

    /// Connect to the USB device: initialize device handle, spawn read task.
    pub async fn connect(&self) -> Result<(), crate::Error> {
        let mut active_guard = self.active.write().await;

        // Already connected
        if active_guard.is_some() {
            return Ok(());
        }

        let handle = Arc::new(self.initialize_device_handle()?);
        let (shutdown, _) = broadcast::channel(1);

        let task_handle = handle.clone();
        let mut task_shutdown = shutdown.subscribe();
        let (message_tx, message_rx) = mpsc::channel(MESSAGE_CHANNEL_SIZE);

        // Spawn background task to read from USB and forward messages
        let message_rx_task = tokio::task::spawn_blocking(move || {
            let mut message_buffer = Vec::<u8>::new();
            let mut escaped = false;
            let timeout = Duration::from_millis(100);
            loop {
                let mut read_buffer = [u8::default(); USB_BUFFER_SIZE];
                let read_result = task_handle.read_bulk(Self::READ_ENDPOINT, &mut read_buffer, timeout)
                        .map_err(|e| crate::Error::USB(e));

                if let Ok(bytes_read) = read_result {
                    for i in 0..bytes_read {
                        let byte = read_buffer[i];

                        if message_buffer.is_empty() && byte != serialization::CONTROL_CHAR_STX {
                            continue;
                        }

                        message_buffer.push(byte);

                        if byte == serialization::CONTROL_CHAR_ETX && !escaped {
                            match Message::<(), ()>::try_from(message_buffer.clone()) {
                                Ok(message) => {
                                    log::trace!("received message: {:?}", message);
                                    if let Err(_e) = message_tx.try_send(message) {}
                                },
                                Err(e) => {
                                    log::trace!("failed to deserialize message: {:?}", e);
                                }
                            }
                            message_buffer.clear();
                        }

                        escaped = byte == serialization::CONTROL_CHAR_DLE && !escaped;
                    }
                }

                if let Ok(_reason) = task_shutdown.try_recv() {
                    log::trace!("USB connection read task shutdown");
                    return Ok(());
                }
            }
        });

        *active_guard = Some(ActiveConnection {
            handle,
            message_rx_task,
            shutdown,
        });
        drop(active_guard);

        // Set up the message receiver separately
        let mut rx_guard = self.message_rx.lock().await;
        *rx_guard = Some(message_rx);

        log::trace!("USB connection established");
        Ok(())
    }

    /// Disconnect from the USB device: stop read task, release interface.
    pub async fn disconnect(&self) -> Result<(), crate::Error> {
        // Clear the message receiver first
        {
            let mut rx_guard = self.message_rx.lock().await;
            *rx_guard = None;
        }

        let mut active_guard = self.active.write().await;

        if let Some(active) = active_guard.take() {
            // Signal the background task to shutdown
            let _ = active.shutdown.send(());

            // Wait for the task to complete
            match active.message_rx_task.await {
                Ok(Ok(())) => log::trace!("USB read task stopped cleanly"),
                Ok(Err(e)) => log::warn!("USB read task ended with error: {:?}", e),
                Err(e) => log::warn!("Failed to join USB read task: {:?}", e),
            }

            // Release the USB interface
            if let Err(e) = active.handle.release_interface(Self::USB_INTERFACE) {
                log::warn!("Failed to release USB interface: {:?}", e);
            }

            log::trace!("USB connection torn down");
        }

        Ok(())
    }

    /// Create a RemoteRuntime wrapping this connection.
    pub fn into_runtime(self) -> RemoteRuntime {
        RemoteRuntime::new(Arc::new(self))
    }

    /// Get the underlying USB device.
    pub fn device(&self) -> USBDevice {
        self.device.clone()
    }

    /// Get the device serial number.
    pub fn device_serial(&self) -> Result<String, crate::Error> {
        let active_guard = self.active.blocking_read();
        let active = active_guard.as_ref()
            .ok_or_else(|| crate::Error::Debug("not connected".to_string()))?;
        let descriptor = active.handle.device().device_descriptor()
            .map_err(|usb_error| crate::Error::USB(usb_error))?;
        active.handle.read_serial_number_string_ascii(&descriptor)
            .map_err(|usb_error| crate::Error::USB(usb_error))
    }

}

#[async_trait]
impl RemoteRuntimeConnection for USBConnection {
    fn is_connected(&self) -> bool {
        self.active.try_read()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    async fn connect(&self) -> Result<(), crate::Error> {
        USBConnection::connect(self).await
    }

    async fn disconnect(&self) -> Result<(), crate::Error> {
        USBConnection::disconnect(self).await
    }

    async fn send_message(&self, message: Message<(), ()>) -> Result<(), crate::Error> {
        let active_guard = self.active.read().await;
        let active = active_guard.as_ref()
            .ok_or_else(|| crate::Error::Debug("not connected".to_string()))?;
        let payload: Vec<u8> = Vec::<u8>::try_from(message)
            .map_err(|_| crate::Error::Serialization)?;
        active.handle.write_bulk(Self::WRITE_ENDPOINT, &payload, Duration::from_millis(1000))
            .map(|_| ())
            .map_err(|usb_error| crate::Error::USB(usb_error))
    }

    async fn recv_message(&self) -> Result<Message<(), ()>, crate::Error> {
        let mut rx_guard = self.message_rx.lock().await;
        let rx = rx_guard.as_mut()
            .ok_or_else(|| crate::Error::Debug("not connected".to_string()))?;
        
        rx.recv().await
            .ok_or_else(|| crate::Error::Debug("connection closed".to_string()))
    }
}

impl Drop for USBConnection {
    fn drop(&mut self) {
        // Get mutable access to active connection for cleanup
        if let Some(active) = self.active.get_mut().take() {
            // Signal the background task to shutdown
            let _ = active.shutdown.send(());

            // Abort the task to ensure it stops even if blocked on USB read
            active.message_rx_task.abort();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct USBDevice {
    identifier: USBIdentifier,
    device: rusb::Device<rusb::Context>
}

impl USBDevice {
    fn new(identifier: USBIdentifier, device: rusb::Device<rusb::Context>) -> Self {
        Self {
            identifier,
            device
        }
    }

    pub fn identifier(&self) -> USBIdentifier {
        self.identifier
    }

    /// Enumerates all currently attached USB devices that match identifiers in ALL_IDENTIFIERS.
    /// Returns a vector of USBDevice instances for all matching devices found.
    pub fn attached() -> Result<Vec<Self>, crate::Error> {
        let context = rusb::Context::new()
            .map_err(|usb_error| crate::Error::USB(usb_error))?;

        let devices = context.devices()
            .map_err(|usb_error| crate::Error::USB(usb_error))?;

        let mut attached_devices = Vec::new();

        for device in devices.iter() {
            if let Ok(descriptor) = device.device_descriptor() {
                let vendor_id = descriptor.vendor_id();
                let product_id = descriptor.product_id();

                // Check if this device matches any identifier in ALL_IDENTIFIERS
                for identifier in ALL_IDENTIFIERS.iter() {
                    if identifier.vendor_id == vendor_id && identifier.product_id == product_id {
                        attached_devices.push(USBDevice::new(*identifier, device));
                        break;
                    }
                }
            }
        }

        Ok(attached_devices)
    }
}

#[derive(Clone, Debug)]
pub enum USBDeviceEvent {
    Arrived(USBDevice),
    Left(USBDevice),
}

#[derive(Clone, Debug)]
struct USBHotplugHandler {
    usb_identifier: USBIdentifier,
    event_tx: mpsc::Sender<USBDeviceEvent>
}

impl USBHotplugHandler {
    fn new(usb_identifier: USBIdentifier, event_tx: mpsc::Sender<USBDeviceEvent>) -> Self {
        Self {
            usb_identifier,
            event_tx
        }
    }
}

impl rusb::Hotplug<rusb::Context> for USBHotplugHandler {
    fn device_arrived(&mut self, arrived_device: rusb::Device<rusb::Context>) {
        log::trace!("{} arrived", self.usb_identifier.name);
        let device = USBDevice::new(
            self.usb_identifier.clone(),
            arrived_device
        );
        let event = USBDeviceEvent::Arrived(device);
        if let Err(e) = self.event_tx.try_send(event) {
            log::error!("failed to send arrived device over event channel: {:?}", e);
        }
    }

    fn device_left(&mut self, left_device: rusb::Device<rusb::Context>) {
        log::trace!("{} left", self.usb_identifier.name);
        let device = USBDevice::new(
            self.usb_identifier.clone(),
            left_device
        );
        let event = USBDeviceEvent::Left(device);
        if let Err(e) = self.event_tx.try_send(event) {
            log::error!("failed to send left device over event channel: {:?}", e);
        }
    }
}

#[derive(Debug)]
pub struct USBDeviceMonitor {
    connected_devices: Arc<std::sync::Mutex<Vec<USBDevice>>>,
    device_event_task: Option<JoinHandle<Result<(), crate::Error>>>,
    hotplug_registrations: Vec<rusb::Registration<rusb::Context>>,
    hotplug_monitor_task: Option<JoinHandle<Result<(), crate::Error>>>,
    task_shutdown: broadcast::Sender<()>,
    usb_context: Option<rusb::Context>,
}

impl USBDeviceMonitor {
    pub fn new() -> Self {
        let (task_shutdown, _) = broadcast::channel(1);
        Self {
            connected_devices: Arc::new(std::sync::Mutex::new(Vec::new())),
            device_event_task: None,
            hotplug_registrations: Vec::new(),
            hotplug_monitor_task: None,
            task_shutdown,
            usb_context: None
        }
    }

    pub fn start(&mut self, event_listener: Option<mpsc::Sender<USBDeviceEvent>>) -> Result<(), crate::Error> {
        // configure the rusb context
        let mut usb_context = rusb::Context::new().map_err(|usb_error| crate::Error::USB(usb_error))?;
        usb_context.set_log_level(RUSB_LOG_LEVEL);
        usb_context.set_log_callback(Box::new(rusb_log_shim), rusb::LogCallbackMode::Context);
        self.usb_context = Some(usb_context.clone());


        // install handlers for all usb vendor / product combos
        let (device_event_tx, mut device_event_rx) = mpsc::channel(4);
        for usb_identifier in ALL_IDENTIFIERS {
            let handler = USBHotplugHandler::new(usb_identifier.clone(), device_event_tx.clone());
            let registration = rusb::HotplugBuilder::new()
                .vendor_id(usb_identifier.vendor_id)
                .product_id(usb_identifier.product_id)
                .enumerate(true)
                .register(usb_context.clone(), Box::new(handler))
                .map_err(|usb_error| crate::Error::USB(usb_error))?;
            self.hotplug_registrations.push(registration);
        }

        // spawn a task to monitor the rusb context
        let task_usb_context = usb_context.clone();
        let mut task_shutdown = self.task_shutdown.subscribe();
        let hotplug_monitor_task = tokio::task::spawn_blocking(move || {
            let timeout = Some(Duration::from_secs(1));
            loop {
                task_usb_context.handle_events(timeout).map_err(|usb_error| crate::Error::USB(usb_error))?;
                if let Ok(_reason) = task_shutdown.try_recv() {
                    log::trace!("hotplug monitor task shutdown");
                    return Ok(());
                };
            }
        });
        self.hotplug_monitor_task = Some(hotplug_monitor_task);

        // spawn a task to listen for USBDeviceEvents
        let mut task_shutdown = self.task_shutdown.subscribe();
        let task_connected_devices = self.connected_devices.clone();
        let device_event_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(event) = device_event_rx.recv() => {
                        // maintain the connected device map
                        match event.clone() {
                            USBDeviceEvent::Arrived(device) => {
                                let mut devices = task_connected_devices.lock().unwrap();
                                devices.push(device);
                                log::trace!("device added, count: {}", devices.len());
                            },
                            USBDeviceEvent::Left(device) => {
                                let mut devices = task_connected_devices.lock().unwrap();
                                devices.retain(|d| d != &device);
                                log::trace!("device removed, remaining: {}", devices.len());
                            }
                        }

                        // notify the listener
                        if let Some(event_listener) = &event_listener {
                            if let Err(send_error) = event_listener.send(event).await {
                                log::error!("failed to send USBDeviceEvent to listener: {:?}", send_error);
                            }
                        }
                    },
                    Ok(_) = task_shutdown.recv() => {
                        log::trace!("device event task shutdown");
                        return Ok(());
                    }
                }
            }
        });
        self.device_event_task = Some(device_event_task);

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), crate::Error> {
        // notify all child tasks to shutdown
        if let Err(_e) = self.task_shutdown.send(()) {
            let error = crate::Error::Debug("failed to send USBDeviceMonitor shutdown".to_string());
            return Err(error);
        }

        // wait for all tasks
        if let Some(task) = self.hotplug_monitor_task.take() {
            let task_result = task.await;
            match task_result {
                Ok(Err(e)) => { return Err(e); },
                Err(_e) => { return Err(crate::Error::Debug("failed to join hotplug monitor task".to_string())); },
                _ => {}
            };
        }

        if let Some(task) = self.device_event_task.take() {
            let task_result = task.await;
            match task_result {
                Ok(Err(e)) => { return Err(e); },
                Err(_e) => { return Err(crate::Error::Debug("failed to join device event task".to_string())); },
                _ => {}
            };
        }

        Ok(())
    }

    pub fn connected_devices(&mut self) -> Vec<USBDevice> {
        self.connected_devices.lock().unwrap().clone()
    }

    pub fn shutdown_rx(&mut self) -> broadcast::Receiver<()> {
        self.task_shutdown.subscribe()
    }
}

/// USB-based environment for discovering and managing USB-connected Enody devices.
///
/// USBEnvironment implements the DiscoveryEnvironment trait, providing:
/// - Automatic device discovery on construction via USB enumeration
/// - Continuous discovery via USB hotplug monitoring
/// - Creation of RemoteRuntime objects for connected devices
pub struct USBEnvironment {
    id: Identifier,
    monitor: Option<USBDeviceMonitor>,
    runtimes: Arc<std::sync::Mutex<Vec<RemoteRuntime>>>,
    /// Store connections to track device identity for removal
    connections: Arc<std::sync::Mutex<Vec<Arc<USBConnection>>>>,
    event_handler_task: Option<JoinHandle<()>>,
    discovery_running: bool,
}

impl USBEnvironment {
    /// Create a new USBEnvironment and enumerate all currently attached devices.
    pub fn new() -> Self {
        let runtimes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let connections = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Enumerate all currently attached devices
        Self::enumerate_attached(&runtimes, &connections);

        Self {
            id: Identifier::new_v4(),
            monitor: None,
            runtimes,
            connections,
            event_handler_task: None,
            discovery_running: false,
        }
    }

    /// Enumerate all currently attached USB devices and connect to them.
    /// This is called during construction and can be called to refresh the device list.
    fn enumerate_attached(
        runtimes: &Arc<std::sync::Mutex<Vec<RemoteRuntime>>>,
        connections: &Arc<std::sync::Mutex<Vec<Arc<USBConnection>>>>
    ) {
        let devices = match USBDevice::attached() {
            Ok(devices) => devices,
            Err(e) => {
                log::warn!("Failed to enumerate USB devices: {:?}", e);
                return;
            }
        };

        let mut runtimes_guard = runtimes.lock().unwrap();
        let mut connections_guard = connections.lock().unwrap();

        for device in devices {
            let usb_connection = USBConnection::new(device);
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(usb_connection.connect())
            });
            match result {
                Ok(()) => {
                    log::trace!("Connected to USB device");
                    let connection = Arc::new(usb_connection);
                    let runtime = RemoteRuntime::new(connection.clone());
                    connections_guard.push(connection);
                    runtimes_guard.push(runtime);
                }
                Err(e) => {
                    log::warn!("Failed to connect to USB device: {:?}", e);
                }
            }
        }
    }

    /// Handle a USB device arrival event by connecting and adding the runtime.
    fn handle_device_arrived(
        device: USBDevice,
        runtimes: &Arc<std::sync::Mutex<Vec<RemoteRuntime>>>,
        connections: &Arc<std::sync::Mutex<Vec<Arc<USBConnection>>>>
    ) {
        let usb_connection = USBConnection::new(device);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(usb_connection.connect())
        });
        match result {
            Ok(()) => {
                let mut runtimes_guard = runtimes.lock().unwrap();
                let mut connections_guard = connections.lock().unwrap();
                log::trace!("Device arrived, connected. Total runtimes: {}", runtimes_guard.len() + 1);
                let connection = Arc::new(usb_connection);
                let runtime = RemoteRuntime::new(connection.clone());
                connections_guard.push(connection);
                runtimes_guard.push(runtime);
            }
            Err(e) => {
                log::warn!("Failed to connect to arrived USB device: {:?}", e);
            }
        }
    }

    /// Handle a USB device departure event by removing the corresponding runtime.
    fn handle_device_left(
        device: USBDevice,
        runtimes: &Arc<std::sync::Mutex<Vec<RemoteRuntime>>>,
        connections: &Arc<std::sync::Mutex<Vec<Arc<USBConnection>>>>
    ) {
        let mut runtimes_guard = runtimes.lock().unwrap();
        let mut connections_guard = connections.lock().unwrap();

        // Find the index of the connection for this device
        if let Some(index) = connections_guard.iter().position(|conn| conn.device() == device) {
            connections_guard.remove(index);
            runtimes_guard.remove(index);
        }

        log::trace!("Device left. Remaining runtimes: {}", runtimes_guard.len());
    }
}

impl Default for USBEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment for USBEnvironment {
    fn identifier(&self) -> Identifier {
        self.id
    }

    fn runtimes(&self) -> Vec<RemoteRuntime> {
        self.runtimes.lock().unwrap().clone()
    }
}

#[async_trait(?Send)]
impl DiscoveryEnvironment for USBEnvironment {
    async fn start_discovery(&mut self) -> Result<(), crate::Error> {
        if self.discovery_running {
            return Ok(());
        }

        // Create event channel for hotplug events
        let (event_tx, mut event_rx) = mpsc::channel::<USBDeviceEvent>(16);

        // Create and start the USB device monitor
        let mut monitor = USBDeviceMonitor::new();
        monitor.start(Some(event_tx))?;
        self.monitor = Some(monitor);
        self.discovery_running = true;

        // Spawn a task to handle device events
        let runtimes = self.runtimes.clone();
        let connections = self.connections.clone();
        let event_handler_task = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    USBDeviceEvent::Arrived(device) => {
                        Self::handle_device_arrived(device, &runtimes, &connections);
                    }
                    USBDeviceEvent::Left(device) => {
                        Self::handle_device_left(device, &runtimes, &connections);
                    }
                }
            }
        });
        self.event_handler_task = Some(event_handler_task);

        Ok(())
    }

    async fn stop_discovery(&mut self) -> Result<(), crate::Error> {
        if !self.discovery_running {
            return Ok(());
        }

        // Stop the monitor (this will close the event channel)
        if let Some(mut monitor) = self.monitor.take() {
            monitor.stop().await?;
        }

        // Abort the event handler task
        if let Some(task) = self.event_handler_task.take() {
            task.abort();
        }

        self.discovery_running = false;

        Ok(())
    }
}
