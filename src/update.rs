use crate::{
    environment::Environment,
    host::remote::RemoteHost,
    message::HostInfo,
    runtime::remote::RemoteRuntime,
    usb::serialport::{SerialPortBackend, SerialPortDevice},
    usb::UsbEnvironment,
    Error, Identifier,
};
use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(1);
const FIRMWARE_FLASH_OFFSET: u32 = 0x0002_0000;

#[derive(Clone)]
pub struct EP01UpdateTarget {
    info: HostInfo,
    ports: Vec<(String, serialport::UsbPortInfo)>,
}

impl EP01UpdateTarget {
    /// Discover attached devices without querying firmware. Uses the USB
    /// serial number (MAC address) to deduplicate ports belonging to the same
    /// physical device. Returns targets with a nil host UUID and zero
    /// version — useful when firmware is broken and cannot respond to
    /// HostInfo queries.
    pub fn attached_raw() -> Vec<Self> {
        let Ok(matches) = SerialPortBackend::attached_ports() else {
            return vec![];
        };

        let mut hosts = Vec::<Self>::new();
        for (port_name, port_info) in matches {
            let serial = port_info.serial_number.as_deref();

            if let Some(existing) = serial.and_then(|sn| {
                hosts.iter_mut().find(|h| {
                    h.ports
                        .iter()
                        .any(|(_, pi)| pi.serial_number.as_deref() == Some(sn))
                })
            }) {
                existing.ports.push((port_name, port_info));
                continue;
            }

            let info = HostInfo {
                version: crate::message::Version::new(0, 0, 0),
                identifier: Identifier::nil(),
            };
            hosts.push(EP01UpdateTarget {
                info,
                ports: vec![(port_name, port_info)],
            });
        }

        hosts
    }

    /// Discover attached devices and query each for its HostInfo.
    ///
    /// Calls [`attached_raw`](Self::attached_raw) to enumerate and
    /// deduplicate by serial number, then connects to each target to
    /// retrieve the firmware-reported host identifier and version.
    /// Targets that fail to respond retain the nil/zero defaults from
    /// `attached_raw`.
    pub async fn attached() -> Vec<Self> {
        let mut targets = Self::attached_raw();

        for target in &mut targets {
            let Some((port_name, port_info)) = target.ports.first() else {
                continue;
            };

            let device = SerialPortDevice::new(port_name.clone(), port_info.serial_number.clone());
            let runtime = RemoteRuntime::new(Box::new(device));
            if runtime.connect().await.is_err() {
                log::warn!("failed to connect to device: {:?}", port_info.serial_number);
                continue;
            }

            let host_result = tokio::time::timeout(
                CONNECTION_TIMEOUT,
                RemoteHost::from_runtime(runtime.clone()),
            )
            .await;

            let _ = runtime.disconnect().await;

            match host_result {
                Ok(Ok(host)) => {
                    target.info = HostInfo {
                        version: host.version(),
                        identifier: host.identifier(),
                    };
                }
                _ => {
                    log::warn!(
                        "failed to collect HostInfo from device: {:?}",
                        port_info.serial_number
                    );
                }
            }
        }

        targets
    }

    pub fn info(&self) -> &HostInfo {
        &self.info
    }

    /// USB serial number of the device, on EP01 this is the ESP32-C6 MAC address
    pub fn mac_address(&self) -> Option<&str> {
        self.ports
            .first()
            .and_then(|(_, pi)| pi.serial_number.as_deref())
    }

    fn preferred_port(&self) -> (String, serialport::UsbPortInfo) {
        let mut sorted = self.ports.clone();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));

        if let Some(preferred) = sorted.iter().find(|(name, _)| name.contains("/cu.")) {
            return preferred.clone();
        }

        sorted[0].clone()
    }

    pub fn flash_firmware_image(&self, firmware_path: &Path) -> Result<(), Error> {
        use espflash::cli::EspflashProgress;
        use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
        use espflash::flasher::Flasher;
        use espflash::target::Chip;
        use serialport::FlowControl;

        let (port_name, port_info) = self.preferred_port();

        let image = std::fs::read(firmware_path).map_err(|e| {
            Error::Debug(format!(
                "Failed to read firmware image {}: {e}",
                firmware_path.display()
            ))
        })?;

        let serial = serialport::new(&port_name, 115_200)
            .flow_control(FlowControl::None)
            .open_native()
            .map_err(|e| Error::Debug(format!("Failed to open serial port {}: {e}", port_name)))?;

        let connection = Connection::new(
            serial,
            port_info,
            ResetAfterOperation::HardReset,
            ResetBeforeOperation::DefaultReset,
            115_200,
        );
        let mut flasher =
            Flasher::connect(connection, true, false, false, Some(Chip::Esp32c6), None).map_err(
                |e| {
                    Error::Debug(format!(
                        "Failed to connect with espflash on {}: {:?}",
                        port_name, e
                    ))
                },
            )?;

        let mut progress = EspflashProgress::default();

        flasher
            .write_bin_to_flash(FIRMWARE_FLASH_OFFSET, &image, &mut progress)
            .map_err(|e| Error::Debug(format!("Failed to flash firmware: {:?}", e)))?;

        Ok(())
    }

    async fn verify_updated_host(&self) -> Result<(), Error> {
        let timeout = Duration::from_secs(30);
        let interval = Duration::from_secs(3);
        let start = Instant::now();
        let host_identifier = self.info.identifier;

        loop {
            let environment = UsbEnvironment::new();
            for runtime in environment.runtimes() {
                let Ok(host) = runtime.host().await else {
                    continue;
                };

                if host.identifier() == host_identifier {
                    println!(
                        "Verified hostInfo for {} (version {}).",
                        host.identifier(),
                        host.version()
                    );
                    return Ok(());
                }
            }

            if start.elapsed() >= timeout {
                break;
            }

            tokio::time::sleep(interval).await;
        }

        Err(Error::Debug(
            "Timed out waiting for update verification.".into(),
        ))
    }
}

fn firmware_path(host_identifier: Identifier, firmware: Option<PathBuf>) -> Result<PathBuf, Error> {
    let Some(path) = firmware else {
        return Err(Error::Debug(format!(
            "Firmware download backend is not yet implemented for host {}. Supply --firmware <FILE>.",
            host_identifier
        )));
    };

    let absolute_path = std::fs::canonicalize(&path).map_err(|e| {
        Error::Debug(format!(
            "Failed to resolve firmware image path {}: {e}",
            path.display()
        ))
    })?;
    if !absolute_path.is_file() {
        return Err(Error::Debug(format!(
            "Firmware image path is not a file: {}",
            absolute_path.display()
        )));
    }

    Ok(absolute_path)
}

fn select_update_target(mut hosts: Vec<EP01UpdateTarget>) -> Result<EP01UpdateTarget, Error> {
    if hosts.is_empty() {
        return Err(Error::Debug("No EP01 devices available.".into()));
    }

    // print selection prompt to screen
    println!("Available devices:");
    for (idx, host) in hosts.iter().enumerate() {
        println!(
            "  {}. {} (version {})",
            idx + 1,
            host.info.identifier,
            host.info.version
        );
    }
    let selection_counter = if hosts.len() > 1 {
        format!(" [1-{}", hosts.len())
    } else {
        "".into()
    };
    print!("Select device{}: ", selection_counter);
    io::stdout()
        .flush()
        .map_err(|e| Error::Debug(format!("Failed to flush stdout for selection prompt: {e}")))?;

    // wait for user selection
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| Error::Debug(format!("Failed to read host selection: {e}")))?;

    // parse user input
    let parsed = line.trim().parse::<usize>().map_err(|_| {
        Error::Debug(format!(
            "Invalid selection '{}'. Expected a number between 1 and {}.",
            line.trim(),
            hosts.len()
        ))
    })?;

    if parsed < 1 || parsed > hosts.len() {
        return Err(Error::Debug(format!(
            "Selection {} is out of range. Expected 1..={}.",
            parsed,
            hosts.len()
        )));
    }

    let selected_index = parsed - 1;
    Ok(hosts.remove(selected_index))
}

pub async fn update_remote_host(firmware: Option<PathBuf>) -> Result<(), Error> {
    let hosts = EP01UpdateTarget::attached().await;

    let selected_host = select_update_target(hosts)?;
    let host_info = selected_host.info.clone();
    let host_identifier = host_info.identifier;
    let current_version = host_info.version.to_string();
    println!(
        "Updating host {} (current version {}).",
        host_identifier, current_version
    );

    let firmware_path = firmware_path(host_identifier, firmware)?;
    println!("Using firmware image {}.", firmware_path.display());

    println!("Flashing selected device...");
    selected_host.flash_firmware_image(&firmware_path)?;

    println!("Flash complete. Verifying update...");
    selected_host.verify_updated_host().await?;
    println!("Update verified.");

    Ok(())
}
