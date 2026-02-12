use crate::{
    environment::Environment,
    message::HostInfo,
    runtime::remote::RemoteRuntime,
    usb::serialport::{SerialPortBackend, SerialPortDevice},
    usb::UsbEnvironment,
    Error,
    Identifier,
};
use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone)]
struct EP01UpdateTarget {
    info: HostInfo,
    ports: Vec<(String, serialport::UsbPortInfo)>,
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

async fn probe_host_info_with_serial_device(
    port_name: &str,
    serial_number: Option<String>,
    timeout: Duration,
) -> Result<Option<HostInfo>, Error> {
    let device = SerialPortDevice::new(port_name.to_string(), serial_number);
    let runtime = RemoteRuntime::new(Box::new(device));
    runtime.connect().await.map_err(|e| {
        Error::Debug(format!(
            "Failed to connect serial device {}: {:?}",
            port_name, e
        ))
    })?;

    let result = tokio::time::timeout(timeout, runtime.host()).await;
    let _ = runtime.disconnect().await;

    match result {
        Ok(Ok(host)) => Ok(Some(HostInfo {
            version: host.version(),
            identifier: host.identifier(),
        })),
        Ok(Err(e)) => Err(Error::Debug(format!(
            "Failed to query HostInfo on {}: {:?}",
            port_name, e
        ))),
        Err(_) => Ok(None),
    }
}

fn preferred_port_for_host(
    ports: &[(String, serialport::UsbPortInfo)],
) -> (String, serialport::UsbPortInfo) {
    let mut sorted = ports.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    if let Some(preferred) = sorted.iter().find(|(name, _)| name.contains("/cu.")) {
        return preferred.clone();
    }

    sorted[0].clone()
}

fn select_ep01_host(mut hosts: Vec<EP01UpdateTarget>) -> Result<EP01UpdateTarget, Error> {
    if hosts.is_empty() {
        return Err(Error::Debug(
            "No EP01 devices responded to HostInfo within 1 second.".into(),
        ));
    }

    hosts.sort_by(|a, b| a.info.identifier.to_string().cmp(&b.info.identifier.to_string()));
    if hosts.len() == 1 {
        return Ok(hosts.remove(0));
    }

    println!("Multiple EP01 devices responded to HostInfo. Select one:");
    for (idx, host) in hosts.iter().enumerate() {
        println!(
            "  {}. {} (version {})",
            idx + 1,
            host.info.identifier,
            host.info.version
        );
    }

    print!("Enter selection [1-{}] (default: 1): ", hosts.len());
    io::stdout()
        .flush()
        .map_err(|e| Error::Debug(format!("Failed to flush stdout for selection prompt: {e}")))?;

    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| Error::Debug(format!("Failed to read host selection: {e}")))?;

    let selected_index = if line.trim().is_empty() {
        0
    } else {
        let parsed = line.trim().parse::<usize>().map_err(|_| {
            Error::Debug(format!(
                "Invalid selection '{}'. Expected a number between 1 and {}.",
                line.trim(),
                hosts.len()
            ))
        })?;
        if !(1..=hosts.len()).contains(&parsed) {
            return Err(Error::Debug(format!(
                "Selection {} is out of range. Expected 1..={}.",
                parsed,
                hosts.len()
            )));
        }
        parsed - 1
    };

    Ok(hosts.remove(selected_index))
}

fn flash_firmware_image(
    port_name: &str,
    port_info: serialport::UsbPortInfo,
    firmware_path: &Path,
) -> Result<(), Error> {
    use espflash::cli::EspflashProgress;
    use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
    use espflash::flasher::Flasher;
    use espflash::target::Chip;
    use serialport::FlowControl;

    let image = std::fs::read(firmware_path).map_err(|e| {
        Error::Debug(format!(
            "Failed to read firmware image {}: {e}",
            firmware_path.display()
        ))
    })?;

    let serial = serialport::new(port_name, 115_200)
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
    let mut flasher = Flasher::connect(connection, true, false, false, Some(Chip::Esp32c6), None)
        .map_err(|e| {
            Error::Debug(format!(
                "Failed to connect with espflash on {}: {:?}",
                port_name, e
            ))
        })?;

    let mut progress = EspflashProgress::default();

    flasher
        .write_bin_to_flash(0x20000, &image, &mut progress)
        .map_err(|e| Error::Debug(format!("Failed to flash firmware at 0x20000: {:?}", e)))?;

    Ok(())
}

async fn verify_updated_host(host_identifier: Identifier) -> Result<(), Error> {
    let timeout = Duration::from_secs(30);
    let interval = Duration::from_secs(3);
    let start = Instant::now();

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

    Err(Error::Debug(format!(
        "Timed out after 30 seconds waiting for hostInfo from {}.",
        host_identifier
    )))
}

pub async fn update_remote_host(firmware: Option<PathBuf>) -> Result<(), Error> {
    // First detect all available EP01 units. This is done by finding all
    // available ESP32-C6 serial devices and attempting to collect HostInfo.
    let matches = SerialPortBackend::attached_ports()?;
    if matches.is_empty() {
        return Err(Error::USB("No EP01 detected".into()));
    }

    let mut hosts = Vec::<EP01UpdateTarget>::new();
    for (port_name, port_info) in matches {
        match probe_host_info_with_serial_device(
            &port_name,
            port_info.serial_number.clone(),
            Duration::from_secs(1),
        )
        .await
        {
            Ok(Some(host_info)) => {
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
            Ok(None) => {}
            Err(error) => log::debug!("HostInfo probe failed on {}: {:?}", port_name, error),
        }
    }

    let selected_host = select_ep01_host(hosts)?;
    let host_info = selected_host.info;
    let (port_name, port_info) = preferred_port_for_host(&selected_host.ports);
    let host_identifier = host_info.identifier;
    let current_version = host_info.version.to_string();
    println!(
        "Updating host {} (current version {}).",
        host_identifier, current_version
    );

    let firmware_path = firmware_path(host_identifier, firmware)?;
    println!("Using firmware image {}.", firmware_path.display());

    println!("Flashing selected device at offset 0x20000...");
    flash_firmware_image(&port_name, port_info, &firmware_path)?;

    println!("Flash complete. Verifying hostInfo every 3s for up to 30s...");
    verify_updated_host(host_identifier).await?;
    println!("Update verified.");

    Ok(())
}
