use async_trait::async_trait;
use std::{
    collections::HashSet,
    fmt::Debug,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{mpsc, Mutex as AsyncMutex},
    task::JoinHandle,
};

use crate::{
    environment::{DiscoveryEnvironment, Environment, EnvironmentRuntimeEvent},
    runtime::remote::{RemoteRuntime, RemoteRuntimeConnection},
    Identifier,
};

pub mod rusb;
pub mod serialport;

use rusb::RusbBackend;
use serialport::SerialPortBackend;

const USB_BUFFER_SIZE: usize = 1024;
const MESSAGE_CHANNEL_SIZE: usize = 64;
const USB_EVENT_CHANNEL_SIZE: usize = 16;
const RUNTIME_EVENT_CHANNEL_SIZE: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct UsbIdentifier {
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
}

impl PartialEq for UsbIdentifier {
    fn eq(&self, other: &Self) -> bool {
        self.vendor_id == other.vendor_id && self.product_id == other.product_id
    }
}

pub(crate) const EP01: UsbIdentifier = UsbIdentifier {
    name: "EP01",
    vendor_id: 0x303A,
    product_id: 0x1001,
};

pub(crate) const ALL_IDENTIFIERS: [UsbIdentifier; 1] = [EP01];

pub trait UsbDevice: RemoteRuntimeConnection {
    fn identifier(&self) -> UsbIdentifier;
    fn serial_number(&self) -> Option<String>;
    fn connection_key(&self) -> String;
}

#[derive(Debug)]
pub enum UsbDeviceEvent {
    Connected(Box<dyn UsbDevice>),
    Disconnected(Box<dyn UsbDevice>),
}

pub(crate) trait UsbBackend: Debug {
    fn init(device_event_tx: mpsc::Sender<UsbDeviceEvent>) -> Result<Self, crate::Error>
    where
        Self: Sized;
    fn attached(&self) -> Result<Vec<Box<dyn UsbDevice>>, crate::Error>;
    fn start_discovery(&mut self) -> Result<(), crate::Error>;
    fn stop_discovery(&mut self) -> Result<(), crate::Error>;
}

#[derive(Clone, Debug)]
struct ConnectedRuntime {
    runtime: RemoteRuntime,
    connection_key: String,
    serial_number: Option<String>,
}

/// USB-based environment for discovering and managing USB-connected Enody devices.
pub struct UsbEnvironment {
    backends: Vec<Box<dyn UsbBackend>>,
    identifier: Identifier,
    connected_runtimes: Arc<Mutex<Vec<ConnectedRuntime>>>,
    device_event_rx: Option<mpsc::Receiver<UsbDeviceEvent>>,
    runtime_event_tx: mpsc::Sender<EnvironmentRuntimeEvent>,
    runtime_event_rx: AsyncMutex<mpsc::Receiver<EnvironmentRuntimeEvent>>,
    event_handler_task: Option<JoinHandle<()>>,
}

impl UsbEnvironment {
    fn enabled_backends(event_tx: mpsc::Sender<UsbDeviceEvent>) -> Vec<Box<dyn UsbBackend>> {
        let mut backends: Vec<Box<dyn UsbBackend>> = Vec::new();

        match RusbBackend::init(event_tx.clone()) {
            Ok(backend) => backends.push(Box::new(backend)),
            Err(e) => log::error!("Failed to initialize RusbBackend: {:?}", e),
        }

        match SerialPortBackend::init(event_tx) {
            Ok(backend) => backends.push(Box::new(backend)),
            Err(e) => log::error!("Failed to initialize SerialPortBackend: {:?}", e),
        }

        backends
    }

    fn connect_runtime(runtime: &RemoteRuntime) -> Result<(), crate::Error> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|e| crate::Error::Debug(format!("tokio runtime required: {e}")))?;
        tokio::task::block_in_place(|| handle.block_on(runtime.connect()))
    }

    fn disconnect_runtime(runtime: &RemoteRuntime) -> Result<(), crate::Error> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|e| crate::Error::Debug(format!("tokio runtime required: {e}")))?;
        tokio::task::block_in_place(|| handle.block_on(runtime.disconnect()))
    }

    fn connected_contains_key(
        connected_runtimes: &[ConnectedRuntime],
        connection_key: &str,
        serial_number: &Option<String>,
    ) -> bool {
        connected_runtimes.iter().any(|connected| {
            connected.connection_key == connection_key
                || serial_number
                    .as_ref()
                    .is_some_and(|sn| connected.serial_number.as_ref() == Some(sn))
        })
    }

    /// Create a new USBEnvironment and enumerate all currently attached devices.
    pub fn new() -> Self {
        let (device_event_tx, device_event_rx) =
            mpsc::channel::<UsbDeviceEvent>(USB_EVENT_CHANNEL_SIZE);
        let (runtime_event_tx, runtime_event_rx) =
            mpsc::channel::<EnvironmentRuntimeEvent>(RUNTIME_EVENT_CHANNEL_SIZE);
        let mut connected_runtimes = Vec::new();
        let mut attached_serials = HashSet::new();

        // Backend order is priority order for initial deduplication.
        let backends = Self::enabled_backends(device_event_tx);
        for backend in &backends {
            let devices = match backend.attached() {
                Ok(devices) => devices,
                Err(e) => {
                    log::warn!("Failed to enumerate attached devices: {:?}", e);
                    continue;
                }
            };

            for device in devices {
                let connection_key = device.connection_key();
                let serial_number = device.serial_number();
                if let Some(serial_number_ref) = serial_number.as_ref() {
                    if !attached_serials.insert(serial_number_ref.clone()) {
                        continue;
                    }
                }

                let runtime = RemoteRuntime::new(device);
                match Self::connect_runtime(&runtime) {
                    Ok(()) => connected_runtimes.push(ConnectedRuntime {
                        runtime,
                        connection_key,
                        serial_number,
                    }),
                    Err(e) => log::warn!("Failed to connect to discovered runtime: {:?}", e),
                }
            }
        }

        Self {
            backends,
            identifier: Identifier::new_v4(),
            connected_runtimes: Arc::new(Mutex::new(connected_runtimes)),
            device_event_rx: Some(device_event_rx),
            runtime_event_tx,
            runtime_event_rx: AsyncMutex::new(runtime_event_rx),
            event_handler_task: None,
        }
    }

    async fn handle_device_arrived(
        connected_runtimes: &Arc<Mutex<Vec<ConnectedRuntime>>>,
        device: Box<dyn UsbDevice>,
    ) -> Option<EnvironmentRuntimeEvent> {
        let connection_key = device.connection_key();
        let serial_number = device.serial_number();

        {
            let connected_guard = connected_runtimes.lock().unwrap();
            if Self::connected_contains_key(&connected_guard, &connection_key, &serial_number) {
                return None;
            }
        }

        let runtime = RemoteRuntime::new(device);
        if let Err(e) = runtime.connect().await {
            log::warn!("Failed to connect to discovered runtime: {:?}", e);
            return None;
        }

        let duplicate = {
            let connected_guard = connected_runtimes.lock().unwrap();
            Self::connected_contains_key(&connected_guard, &connection_key, &serial_number)
        };
        if duplicate {
            if let Err(e) = runtime.disconnect().await {
                log::warn!("Failed to disconnect duplicate discovered runtime: {:?}", e);
            }
            return None;
        }

        let mut connected_guard = connected_runtimes.lock().unwrap();
        connected_guard.push(ConnectedRuntime {
            runtime: runtime.clone(),
            connection_key,
            serial_number,
        });
        Some(EnvironmentRuntimeEvent::Arrived(runtime))
    }

    async fn handle_device_left(
        connected_runtimes: &Arc<Mutex<Vec<ConnectedRuntime>>>,
        device: Box<dyn UsbDevice>,
    ) -> Option<EnvironmentRuntimeEvent> {
        let connection_key = device.connection_key();

        let removed_runtime = {
            let mut connected_guard = connected_runtimes.lock().unwrap();
            let index = connected_guard
                .iter()
                .position(|connected| connected.connection_key == connection_key)?;
            connected_guard.remove(index).runtime
        };

        let removed_runtime_event = removed_runtime.clone();

        if !removed_runtime.is_connected() {
            return Some(EnvironmentRuntimeEvent::Left(removed_runtime_event));
        }

        if let Err(e) = removed_runtime.disconnect().await {
            log::warn!(
                "Failed to disconnect runtime after device removal (key={}): {:?}",
                connection_key,
                e
            );
        }
        Some(EnvironmentRuntimeEvent::Left(removed_runtime_event))
    }
}

impl Default for UsbEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for UsbEnvironment {
    fn drop(&mut self) {
        if let Some(task) = self.event_handler_task.take() {
            task.abort();
        }

        for backend in &mut self.backends {
            if let Err(e) = backend.stop_discovery() {
                log::error!("Failed to stop backend discovery on drop: {:?}", e);
            }
        }

        let runtimes: Vec<RemoteRuntime> = {
            let connected_guard = self.connected_runtimes.lock().unwrap();
            connected_guard
                .iter()
                .map(|connected| connected.runtime.clone())
                .collect()
        };

        for runtime in runtimes {
            if !runtime.is_connected() {
                continue;
            }

            if let Err(e) = Self::disconnect_runtime(&runtime) {
                log::error!(
                    "Failed to disconnect runtime on UsbEnvironment drop: {:?}",
                    e
                );
            }
        }
    }
}

impl Environment for UsbEnvironment {
    fn identifier(&self) -> Identifier {
        self.identifier
    }

    fn runtimes(&self) -> Vec<RemoteRuntime> {
        let connected_guard = self.connected_runtimes.lock().unwrap();
        connected_guard
            .iter()
            .map(|connected| connected.runtime.clone())
            .collect()
    }
}

#[async_trait(?Send)]
impl DiscoveryEnvironment for UsbEnvironment {
    async fn start_discovery(&mut self) -> Result<(), crate::Error> {
        if self.event_handler_task.is_some() {
            return Ok(());
        }

        let mut event_rx = self.device_event_rx.take().ok_or_else(|| {
            crate::Error::Debug("device event channel already consumed".to_string())
        })?;

        let mut started_backends: Vec<usize> = Vec::new();
        for (backend_index, backend) in self.backends.iter_mut().enumerate() {
            if let Err(e) = backend.start_discovery() {
                for started_index in started_backends {
                    let _ = self.backends[started_index].stop_discovery();
                }

                self.device_event_rx = Some(event_rx);
                return Err(crate::Error::Debug(format!(
                    "failed to start discovery for backend {}: {:?}",
                    backend_index, e
                )));
            }

            started_backends.push(backend_index);
        }

        let task_connected_runtimes = Arc::clone(&self.connected_runtimes);
        let task_runtime_event_tx = self.runtime_event_tx.clone();
        self.event_handler_task = Some(tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let runtime_event = match event {
                    UsbDeviceEvent::Connected(device) => {
                        Self::handle_device_arrived(&task_connected_runtimes, device).await
                    }
                    UsbDeviceEvent::Disconnected(device) => {
                        Self::handle_device_left(&task_connected_runtimes, device).await
                    }
                };

                if let Some(runtime_event) = runtime_event {
                    if task_runtime_event_tx.send(runtime_event).await.is_err() {
                        break;
                    }
                }
            }
        }));

        Ok(())
    }

    async fn stop_discovery(&mut self) -> Result<(), crate::Error> {
        if let Some(task) = self.event_handler_task.take() {
            task.abort();
            let _ = task.await;
        }

        let mut first_error = None;
        for backend in &mut self.backends {
            if let Err(e) = backend.stop_discovery() {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }

        Ok(())
    }

    async fn next_runtime_event(&self) -> Result<EnvironmentRuntimeEvent, crate::Error> {
        if self.event_handler_task.is_none() {
            return Err(crate::Error::Debug(
                "discovery is not running; call start_discovery first".to_string(),
            ));
        }

        let mut runtime_event_rx = self.runtime_event_rx.lock().await;
        runtime_event_rx.recv().await.ok_or_else(|| {
            crate::Error::Debug("runtime event stream closed unexpectedly".to_string())
        })
    }
}
