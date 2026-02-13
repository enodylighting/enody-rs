use async_trait::async_trait;
use std::{collections::HashSet, fmt::Debug};

use crate::{
    environment::Environment,
    runtime::remote::{RemoteRuntime, RemoteRuntimeConnection},
    Identifier,
};

pub mod rusb;
pub mod serialport;

use rusb::RusbBackend;
use serialport::SerialPortBackend;

const USB_BUFFER_SIZE: usize = 1024;
const MESSAGE_CHANNEL_SIZE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbIdentifier {
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
}

pub(crate) const EP01: UsbIdentifier = UsbIdentifier {
    name: "EP01",
    vendor_id: 0x303A,
    product_id: 0x1001,
};

pub(crate) const ALL_IDENTIFIERS: [UsbIdentifier; 1] = [EP01];

pub trait UsbDevice<InternalCommand = (), InternalEvent = ()>: RemoteRuntimeConnection {
    fn identifier(&self) -> UsbIdentifier;
    fn serial_number(&self) -> Option<String>;
}

#[derive(Debug)]
pub enum UsbDeviceEvent {
    Arrived(Box<dyn UsbDevice>),
    Left(Box<dyn UsbDevice>),
}

#[async_trait]
pub(crate) trait UsbBackend {
    fn attached(&self) -> Result<Vec<Box<dyn UsbDevice>>, crate::Error>;
    #[allow(dead_code)] // TODO(carter): Dual backend USB device hotplug support
    async fn next_device_event(&self) -> UsbDeviceEvent;
}

/// USB-based environment for discovering and managing USB-connected Enody devices.
pub struct UsbEnvironment {
    _backends: Vec<Box<dyn UsbBackend>>,
    identifier: Identifier,
    runtimes: Vec<RemoteRuntime>,
}

impl UsbEnvironment {
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

    fn enabled_backends() -> Vec<Box<dyn UsbBackend>> {
        vec![
            Box::new(RusbBackend::default()),
            Box::new(SerialPortBackend::default()),
        ]
    }

    /// Create a new USBEnvironment and enumerate all currently attached devices.
    pub fn new() -> Self {
        let mut attached_devices: Vec<Box<dyn UsbDevice>> = Vec::new();
        let mut runtimes = Vec::new();
        let backends = Self::enabled_backends();

        // add all backends available
        for backend in &backends {
            if let Ok(mut devices) = backend.attached() {
                attached_devices.append(&mut devices);
            }
        }

        // deduplicate on serial number
        let mut attached_serials = HashSet::new();
        for device in attached_devices {
            let serial = device.serial_number();
            if let Some(serial) = serial {
                if !attached_serials.insert(serial) {
                    continue;
                }
            }

            let remote_runtime = RemoteRuntime::new(device);
            match Self::connect_runtime(&remote_runtime) {
                Ok(()) => runtimes.push(remote_runtime),
                Err(e) => log::warn!("Failed to connect to discovered runtime: {:?}", e),
            }
        }

        Self {
            _backends: backends,
            identifier: Identifier::new_v4(),
            runtimes,
        }
    }
}

impl Default for UsbEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for UsbEnvironment {
    fn drop(&mut self) {
        for runtime in &self.runtimes {
            if !runtime.is_connected() {
                continue;
            }

            if let Err(e) = Self::disconnect_runtime(runtime) {
                log::warn!(
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
        self.runtimes.clone()
    }
}
