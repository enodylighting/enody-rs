use std::{
    sync::{
        Arc,
        Mutex
    },
    time::Duration
};

use rusb::UsbContext;
use tokio::{
    sync::{
        broadcast,
        mpsc
    },
    task::JoinHandle
};

pub const INFO_CAPACITY: usize = 128;
pub const CACHE_SIZE: usize = 3;
pub const CLIENT_QUEUE_SIZE: usize = 3;
pub const USB_BUFFER_SIZE: usize = 128;
const RUSB_LOG_LEVEL: rusb::LogLevel = rusb::LogLevel::None;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct USBIdentifier {
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
}

#[derive(Clone, Debug)]
enum USBDeviceEvent {
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

    pub fn start(&mut self) -> Result<(), crate::Error> {
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
                        match event {
                            USBDeviceEvent::Arrived(device) => {
                                let mut devices = task_connected_devices.lock().unwrap();
                                devices.push(device);
                                log::trace!("device added, count: {}", devices.len());
                            },
                            USBDeviceEvent::Left(device) => {
                                let mut devices = task_connected_devices.lock().unwrap();
                                let before_count = devices.len();
                                devices.retain(|d| d != &device);
                                log::trace!("device removed, count before: {}, after: {}", before_count, devices.len());
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
            let error = crate::Error::Debug("failed to send USBDeviceMonitor shutdown");
            return Err(error);
        }

        // wait for all tasks
        if let Some(task) = self.hotplug_monitor_task.take() {
            let task_result = task.await;
            match task_result {
                Ok(Err(e)) => { return Err(e); },
                Err(_e) => { return Err(crate::Error::Debug("failed to join hotplug monitor task")); },
                _ => {}
            };
        }

        if let Some(task) = self.device_event_task.take() {
            let task_result = task.await;
            match task_result {
                Ok(Err(e)) => { return Err(e); },
                Err(_e) => { return Err(crate::Error::Debug("failed to join device event task")); },
                _ => {}
            };
        }

        Ok(())
    }

    pub fn connected_devices(&mut self) -> Vec<USBDevice> {
        self.connected_devices.lock().unwrap().clone()
    }
}
