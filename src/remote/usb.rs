use std::{
    fmt::Debug,
    sync::{
        Arc,
        Mutex
    },
    time::Duration
};

use rusb::UsbContext;
use serde::{
    de::DeserializeOwned,
    Serialize
};
use tokio::{
    sync::{
        Mutex as AsyncMutex,
        broadcast,
        mpsc,
        oneshot
    }, task::JoinHandle
};

use crate::{
    message::{
        CommandMessage,
        EventMessage,
        Message
    },
    remote::serialization
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

#[derive(Debug)]
pub struct CommandResponseRegistration<InternalEvent> {
    pub context: crate::Identifier,
    pub response_tx: Option<oneshot::Sender<EventMessage<InternalEvent>>>
}

#[derive(Debug)]
pub struct USBRemoteRuntime<InternalCommand = (), InternalEvent = ()> {
    device: USBDevice,
    handle: Arc<rusb::DeviceHandle<rusb::Context>>,
    message_rx: mpsc::Receiver<crate::message::Message<InternalCommand, InternalEvent>>,
    message_rx_task: Option<JoinHandle<Result<(), crate::Error>>>,
    command_response_registrations: Arc<AsyncMutex<Vec<CommandResponseRegistration<InternalEvent>>>>,
    shutdown: broadcast::Sender<()>
}

impl<InternalCommand, InternalEvent> USBRemoteRuntime<InternalCommand, InternalEvent>
where
    InternalCommand: Clone + Debug + DeserializeOwned + Serialize + Send + Sync + 'static,
    InternalEvent: Clone + Debug + DeserializeOwned + Serialize + Send + Sync + 'static
{
    const USB_INTERFACE: u8 = 1;
    const WRITE_ENDPOINT: u8 = 0x01;
    const READ_ENDPOINT: u8 = 0x81;

    fn initialize_device(device: USBDevice) -> Result<rusb::DeviceHandle<rusb::Context>, crate::Error> {
        let device_handle = device.device.open()
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

    pub fn connect(device: USBDevice) -> Result<Self, crate::Error> {
        let handle = Arc::new(Self::initialize_device(device.clone())?);
        let (shutdown, _) = broadcast::channel(1);
        let command_response_registrations: Arc<AsyncMutex<Vec<CommandResponseRegistration<InternalEvent>>>> =  Arc::new(AsyncMutex::new(Vec::new()));

        let task_handle = handle.clone();
        let mut task_shutdown = shutdown.subscribe();
        let task_command_response_registrations = command_response_registrations.clone();
        let (message_tx, message_rx) = mpsc::channel(MESSAGE_CHANNEL_SIZE);

        let message_rx_task = tokio::task::spawn_blocking(move || {
            let mut message_buffer = Vec::<u8>::new();
            let mut escaped = false;
            let timeout = Duration::from_millis(100);
            loop {
                // read the next chunk of incoming data
                let mut read_buffer = [u8::default(); USB_BUFFER_SIZE];
                let read_result = task_handle.read_bulk(Self::READ_ENDPOINT, &mut read_buffer, timeout)
                        .map_err(|e| crate::Error::USB(e));

                // if the read is succesful, append to the message_buffer
                if let Ok(bytes_read) = read_result {
                    for i in 0..bytes_read {
                        let byte = read_buffer[i];

                        // check if rx started in the middle of a frame
                        if message_buffer.is_empty() && byte != serialization::CONTROL_CHAR_STX {
                            continue;
                        }

                        message_buffer.push(byte);

                        // deserialize if at end of frame
                        if byte == serialization::CONTROL_CHAR_ETX && !escaped {
                            match Message::<InternalCommand, InternalEvent>::try_from(message_buffer.clone()) {
                                Ok(message) => {
                                    // check if any commands were waiting for the response
                                    if let crate::message::Message::Event(event) = &message {
                                        let mut command_response_tx = task_command_response_registrations.blocking_lock();

                                        // First find the matching registration and send the response
                                        if let Some(message_context) = &event.context {
                                            for registration in command_response_tx.iter_mut() {
                                                if registration.context == *message_context {
                                                    if let Some(tx) = registration.response_tx.take() {
                                                        let _ = tx.send(event.clone());
                                                    }
                                                }
                                            }
                                        }

                                        // Then retain only registrations that still have a response_tx
                                        command_response_tx.retain(|registration| {
                                            registration.response_tx.is_some()
                                        });

                                        log::trace!("event: {:?}", event);
                                    }

                                    if let Err(_e) = message_tx.try_send(message) {
                                        // message failed to send, presumable client is not ingesting
                                    }
                                },
                                Err(e) => {
                                    log::trace!("failed to deserialize message: {:?}", e);
                                }
                            }
                            message_buffer.clear();
                        }

                        // check for escaped
                        escaped = byte == serialization::CONTROL_CHAR_DLE && !escaped;
                    }
                }

                if let Ok(_reason) = task_shutdown.try_recv() {
                    log::trace!("remote runtime task shutdown");
                    return Ok(());
                }
            }
        });
        let instance = Self {
            device,
            handle,
            message_rx,
            message_rx_task: Some(message_rx_task),
            command_response_registrations,
            shutdown
        };

        Ok(instance)
    }

    pub fn disconnect(&mut self) -> Result<(), crate::Error> {
        self.shutdown.send(())
            .map_err(|e| { crate::Error::Debug(format!("failed to send shutdown message: {}", e).to_string()) })?;

        if let Some(task) = self.message_rx_task.take() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(task)
            }).map_err(|e| crate::Error::Debug(format!("failed to join message_rx_task: {}", e)))??;
        }

        Ok(())
    }

    fn ensure_device_connected(&self) -> Result<(), crate::Error> {
        Ok(())
    }

    pub fn device(&self) -> USBDevice {
        self.device.clone()
    }

    pub fn device_serial(&self) -> Result<String, crate::Error> {
        let descriptor = self.handle.device().device_descriptor()
            .map_err(|usb_error| crate::Error::USB(usb_error))?;
        self.handle.read_serial_number_string_ascii(&descriptor)
            .map_err(|usb_error| crate::Error::USB(usb_error))
    }

    pub async fn next_message(&mut self) -> Result<crate::message::Message<InternalCommand, InternalEvent>, crate::Error> {
        self.ensure_device_connected()?;

        match self.message_rx.recv().await {
            Some(message) => Ok(message),
            None => Err(crate::Error::Busy)
        }
    }

    pub async fn execute_command(&mut self, command: crate::message::CommandMessage<InternalCommand>) -> Result<EventMessage<InternalEvent>, crate::Error> {
        self.ensure_device_connected()?;

        let context = command.identifier.clone();
        let (response_tx, response_rx) = oneshot::channel();
        let response_registration = CommandResponseRegistration {
            context,
            response_tx: Some(response_tx)
        };

        let mut registration_buffer = self.command_response_registrations.lock().await;
        registration_buffer.push(response_registration);
        drop(registration_buffer);

        self.handle_command(command)?;

        response_rx.await
            .map_err(|_e| crate::Error::Debug("failed waiting for command response event".to_string()))
    }

    pub fn handle_command(&mut self, command: CommandMessage<InternalCommand>) -> Result<(), crate::Error> {
        let message: Message<InternalCommand, InternalEvent> = Message::Command(command);
        let payload: Vec<u8> = Vec::<u8>::try_from(message).unwrap();
        self.handle.write_bulk(Self::WRITE_ENDPOINT, &payload, Duration::ZERO)
            .map(|_| ())
            .map_err(|usb_error| crate::Error::USB(usb_error))
    }

    pub fn handle_event(&mut self, event: EventMessage<InternalEvent>) -> Result<(), crate::Error> {
        let message: Message<InternalCommand, InternalEvent> = Message::Event(event);
        let payload: Vec<u8> = Vec::<u8>::try_from(message).unwrap();
        self.handle.write_bulk(Self::WRITE_ENDPOINT, &payload, Duration::ZERO)
            .map(|_| ())
            .map_err(|usb_error| crate::Error::USB(usb_error))
    }
}

impl<InternalCommand, InternalEvent> Drop for USBRemoteRuntime<InternalCommand, InternalEvent> {
    fn drop(&mut self) {
        // Signal the background task to shutdown
        let _ = self.shutdown.send(());

        // Abort the task to ensure it stops even if blocked on USB read
        if let Some(task) = self.message_rx_task.take() {
            task.abort();
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
    connected_devices: Arc<Mutex<Vec<USBDevice>>>,
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
            connected_devices: Arc::new(Mutex::new(Vec::new())),
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
