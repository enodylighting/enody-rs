use crate::{
    environment::Environment,
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
struct EP01UpdateTarget {
    info: HostInfo,
    ports: Vec<(String, serialport::UsbPortInfo)>,
}

impl EP01UpdateTarget {
    async fn attached() -> Vec<Self> {
        // find all available ESP32-C6 serial devices
        let Ok(matches) = SerialPortBackend::attached_ports() else {
            return vec![];
        };

        // query each device for a HostInfo
        let mut hosts = Vec::<Self>::new();
        for (port_name, port_info) in matches {
            let device =
                SerialPortDevice::new(port_name.to_string(), port_info.serial_number.clone());
            let runtime = RemoteRuntime::new(Box::new(device));
            if runtime.connect().await.is_err() {
                log::warn!(
                    "failed to connect to deivce: {:?}",
                    port_info.serial_number.clone()
                );
                continue;
            }

            // attempt to connect and fetch HostInfo
            let Ok(Ok(host)) = tokio::time::timeout(CONNECTION_TIMEOUT, runtime.host()).await
            else {
                log::warn!(
                    "failed to collect HostInfo from device: {:?}",
                    port_info.serial_number.clone()
                );
                continue;
            };
            let _ = runtime.disconnect().await;

            // check if host is already present on another port
            let host_info = HostInfo {
                version: host.version(),
                identifier: host.identifier(),
            };

            if let Some(existing) = hosts
                .iter_mut()
                .find(|host| host.info.identifier == host_info.identifier)
            {
                existing.ports.push((port_name, port_info));
            } else {
                hosts.push(EP01UpdateTarget {
                    info: host_info,
                    ports: vec![(port_name, port_info)],
                });
            }
        }

        hosts
    }

    fn preferred_port(&self) -> (String, serialport::UsbPortInfo) {
        let mut sorted = self.ports.clone();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));

        if let Some(preferred) = sorted.iter().find(|(name, _)| name.contains("/cu.")) {
            return preferred.clone();
        }

        sorted[0].clone()
    }

    fn flash_firmware_image(&self, firmware_path: &Path) -> Result<(), Error> {
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
