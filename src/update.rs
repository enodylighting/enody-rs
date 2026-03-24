use crate::{
    environment::Environment,
    host::remote::RemoteHost,
    message::{HostInfo, Version},
    runtime::remote::RemoteRuntime,
    usb::serialport::{SerialPortBackend, SerialPortDevice},
    usb::UsbEnvironment,
    Error, Identifier,
};
use serde::Deserialize;
use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(1);
const FIRMWARE_FLASH_OFFSET: u32 = 0x0002_0000;
const FIRMWARE_BASE_URL: &str = "https://firmware.enody.lighting";

#[derive(Clone, Debug, Deserialize)]
struct FirmwarePayload {
    offset: u32,
    length: u32,
    data: String,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FirmwareVersion {
    version: String,
    payload: Vec<FirmwarePayload>,
}

impl FirmwareVersion {
    pub fn version(&self) -> &str {
        &self.version
    }
}

enum ResolvedFirmware {
    LocalFile(PathBuf),
    Payloads(Vec<(u32, Vec<u8>)>),
}

#[derive(Clone, Default)]
pub struct EP01UpdateTarget {
    info: HostInfo,
    ports: Vec<(String, serialport::UsbPortInfo)>,
}

impl EP01UpdateTarget {
    /// On Linux the `cdc_acm` kernel driver must be bound to the CDC ACM
    /// interfaces for `/dev/ttyACMx` to exist. If a previous rusb session
    /// detached the driver (or the VM host never triggered binding in the
    /// first place), we need to kick the driver into life.
    ///
    /// Strategy: open the device via rusb, which detaches the kernel driver
    /// and claims the interface, then immediately close and re-attach. After
    /// this the `cdc_acm` driver binds and the serial port appears.
    #[cfg(target_os = "linux")]
    fn ensure_serial_ports() {
        use crate::usb::rusb::RusbBackend;
        use crate::usb::UsbBackend;

        // Only kick if there are no serial ports yet.
        if SerialPortBackend::attached_ports()
            .map(|p| !p.is_empty())
            .unwrap_or(false)
        {
            return;
        }

        log::debug!("No serial ports found — attempting to bind cdc_acm via rusb cycle");
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);
        let Ok(backend) = RusbBackend::init(event_tx) else {
            return;
        };
        let Ok(devices) = backend.attached() else {
            return;
        };

        // For each matching device, open → claim → release → re-attach drivers.
        // The RusbDeviceConnection::open + close cycle handles this.
        for device in devices {
            // connect() opens the handle, detaches drivers, claims interface
            let rt_handle = match tokio::runtime::Handle::try_current() {
                Ok(h) => h,
                Err(_) => continue,
            };
            if let Err(e) = tokio::task::block_in_place(|| rt_handle.block_on(device.connect())) {
                log::debug!("Failed to connect for driver rebind: {:?}", e);
                continue;
            }
            // disconnect() releases interface and re-attaches kernel drivers
            let _ = tokio::task::block_in_place(|| rt_handle.block_on(device.disconnect()));
        }

        // Give the kernel a moment to bind cdc_acm and create tty devices
        std::thread::sleep(Duration::from_millis(100));

        let count = SerialPortBackend::attached_ports()
            .map(|p| p.len())
            .unwrap_or(0);
        log::debug!("After driver rebind: {} serial port(s) found", count);
    }

    /// Discover attached devices without querying firmware. Uses the USB
    /// serial number (MAC address) to deduplicate ports belonging to the same
    /// physical device. Returns targets with a nil host UUID and zero
    /// version — useful when firmware is broken and cannot respond to
    /// HostInfo queries.
    pub fn attached_raw() -> Vec<Self> {
        // On Linux, ensure the cdc_acm driver is bound so serial ports exist.
        #[cfg(target_os = "linux")]
        Self::ensure_serial_ports();

        let Ok(matches) = SerialPortBackend::attached_ports() else {
            return vec![];
        };

        let mut hosts = Vec::<Self>::new();
        for (port_name, port_info) in matches {
            // if we cannot get a valid serialnumber the USB device is not
            // reliable enough to attempt update
            let Some(serial) = port_info.serial_number.as_deref() else {
                continue;
            };

            // determine if the host already exists
            let mut existing_host = None;
            for host in hosts.iter_mut() {
                for (_, port_info) in &mut host.ports {
                    if Some(serial.into()) == port_info.serial_number {
                        existing_host = Some(host);
                        break;
                    }
                }
                if existing_host.is_some() {
                    break;
                }
            }

            // if the host doesn't exist, add to vec
            if existing_host.is_none() {
                hosts.push(EP01UpdateTarget::default());
                existing_host = hosts.last_mut();
            }

            // add the port
            existing_host.unwrap().ports.push((port_name, port_info));
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
        let image = std::fs::read(firmware_path).map_err(|e| {
            Error::Debug(format!(
                "Failed to read firmware image {}: {e}",
                firmware_path.display()
            ))
        })?;
        self.flash_payloads(&[(FIRMWARE_FLASH_OFFSET, image)])
    }

    pub fn flash_payloads(&self, payloads: &[(u32, Vec<u8>)]) -> Result<(), Error> {
        use espflash::cli::EspflashProgress;
        use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
        use espflash::flasher::Flasher;
        use espflash::target::Chip;
        use serialport::FlowControl;

        let (port_name, port_info) = self.preferred_port();

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

        for (i, (offset, data)) in payloads.iter().enumerate() {
            println!(
                "  Writing payload {}/{} ({} bytes at offset {:#x})...",
                i + 1,
                payloads.len(),
                data.len(),
                offset
            );
            flasher
                .write_bin_to_flash(*offset, data, &mut progress)
                .map_err(|e| Error::Debug(format!("Failed to flash firmware: {:?}", e)))?;
        }

        flasher
            .connection()
            .reset()
            .map_err(|e| Error::Debug(format!("Failed to reset device after flash: {:?}", e)))?;

        Ok(())
    }

    pub async fn available_firmware(&self) -> Result<Vec<FirmwareVersion>, Error> {
        fetch_firmware_manifest(&self.info.identifier).await
    }

    pub async fn update_available(&self) -> Result<bool, Error> {
        let versions = fetch_firmware_manifest(&self.info.identifier).await?;
        let current = &self.info.version;
        Ok(versions
            .iter()
            .any(|fv| fv.version.parse::<Version>().is_ok_and(|v| v > *current)))
    }

    pub async fn update_device(&self, version: &str) -> Result<(), Error> {
        let versions = fetch_firmware_manifest(&self.info.identifier).await?;
        let selected = versions
            .iter()
            .find(|fv| fv.version == version)
            .ok_or_else(|| Error::Debug(format!("Version {} not found", version)))?;
        let payloads = download_firmware_payloads(&self.info.identifier, selected).await?;
        self.flash_payloads(&payloads)?;
        self.verify_updated_host().await
    }

    pub async fn verify_updated_host(&self) -> Result<(), Error> {
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

pub async fn fetch_firmware_manifest(host_id: &Identifier) -> Result<Vec<FirmwareVersion>, Error> {
    let url = format!("{}/{}/firmware.json", FIRMWARE_BASE_URL, host_id);
    println!("Fetching firmware manifest for {}...", host_id);

    let response = reqwest::get(&url).await.map_err(|e| {
        log::debug!("Firmware manifest fetch error: {e}");
        Error::Debug(format!("Failed to fetch firmware manifest from {url}"))
    })?;

    if !response.status().is_success() {
        log::debug!("Firmware manifest status {}: {url}", response.status());
        return Err(Error::Debug(format!(
            "Failed to fetch firmware manifest from {url}"
        )));
    }

    let versions: Vec<FirmwareVersion> = response.json().await.map_err(|e| {
        log::debug!("Firmware manifest parse error: {e}");
        Error::Debug(format!("Failed to parse firmware manifest from {url}"))
    })?;

    Ok(versions)
}

async fn download_payload(
    host_id: &Identifier,
    payload: &FirmwarePayload,
) -> Result<Vec<u8>, Error> {
    let url = format!("{}/{}/{}", FIRMWARE_BASE_URL, host_id, payload.data);
    let response = reqwest::get(&url).await.map_err(|e| {
        log::debug!("Payload download error for {}: {e}", payload.data);
        Error::Debug(format!("Failed to download payload {}", payload.data))
    })?;

    if !response.status().is_success() {
        log::debug!(
            "Payload download status {} for {}",
            response.status(),
            payload.data
        );
        return Err(Error::Debug(format!(
            "Failed to download payload {}",
            payload.data
        )));
    }

    let data = response.bytes().await.map_err(|e| {
        log::debug!("Payload read error for {}: {e}", payload.data);
        Error::Debug(format!("Failed to download payload {}", payload.data))
    })?;

    if data.len() as u32 != payload.length {
        return Err(Error::Debug(format!(
            "Payload {} size mismatch: expected {} bytes, got {}",
            payload.data,
            payload.length,
            data.len()
        )));
    }

    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(&data));
    if hash != payload.sha256 {
        return Err(Error::Debug(format!(
            "Payload {} SHA-256 mismatch: expected {}, got {}",
            payload.data, payload.sha256, hash
        )));
    }

    Ok(data.to_vec())
}

async fn download_firmware_payloads(
    host_id: &Identifier,
    version: &FirmwareVersion,
) -> Result<Vec<(u32, Vec<u8>)>, Error> {
    let mut payloads = Vec::new();
    let total = version.payload.len();

    for (i, fw_payload) in version.payload.iter().enumerate() {
        println!(
            "  Downloading payload {}/{}: {} ({} bytes)...",
            i + 1,
            total,
            fw_payload.data,
            fw_payload.length
        );
        let data = download_payload(host_id, fw_payload).await?;
        payloads.push((fw_payload.offset, data));
    }

    Ok(payloads)
}

fn select_firmware_version(
    versions: &[FirmwareVersion],
    current_version: &Version,
) -> Result<usize, Error> {
    if versions.is_empty() {
        return Err(Error::Debug("No firmware versions available.".into()));
    }

    // Parse and sort indices by version, newest first
    let mut indexed: Vec<(usize, Version)> = versions
        .iter()
        .enumerate()
        .filter_map(|(i, fv)| fv.version.parse::<Version>().ok().map(|v| (i, v)))
        .collect();
    indexed.sort_by(|a, b| b.1.cmp(&a.1));

    if indexed.is_empty() {
        return Err(Error::Debug(
            "No firmware versions with valid semver found.".into(),
        ));
    }

    let recommended_version = &indexed[0].1;

    println!("Available firmware versions:");
    for (display_idx, (orig_idx, ver)) in indexed.iter().enumerate() {
        let mut markers = Vec::new();
        if ver == recommended_version {
            markers.push("recommended");
        }
        if ver == current_version {
            markers.push("current");
        }
        let marker_str = if markers.is_empty() {
            String::new()
        } else {
            format!(" ({})", markers.join(", "))
        };
        println!(
            "  {}. {}{}",
            display_idx + 1,
            versions[*orig_idx].version,
            marker_str
        );
    }

    let range = if indexed.len() > 1 {
        format!(" [1-{}, default: 1]", indexed.len())
    } else {
        " [default: 1]".into()
    };
    print!("Select version{}: ", range);
    io::stdout()
        .flush()
        .map_err(|e| Error::Debug(format!("Failed to flush stdout: {e}")))?;

    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| Error::Debug(format!("Failed to read version selection: {e}")))?;

    let trimmed = line.trim();
    let selection = if trimmed.is_empty() {
        1
    } else {
        trimmed.parse::<usize>().map_err(|_| {
            Error::Debug(format!(
                "Invalid selection '{}'. Expected a number between 1 and {}.",
                trimmed,
                indexed.len()
            ))
        })?
    };

    if selection < 1 || selection > indexed.len() {
        return Err(Error::Debug(format!(
            "Selection {} is out of range. Expected 1..={}.",
            selection,
            indexed.len()
        )));
    }

    Ok(indexed[selection - 1].0)
}

async fn resolve_firmware(
    host_identifier: Identifier,
    current_version: &Version,
    firmware: Option<PathBuf>,
) -> Result<ResolvedFirmware, Error> {
    if let Some(path) = firmware {
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
        return Ok(ResolvedFirmware::LocalFile(absolute_path));
    }

    let versions = fetch_firmware_manifest(&host_identifier).await?;
    let selected_idx = select_firmware_version(&versions, current_version)?;
    let selected = &versions[selected_idx];

    println!("Downloading firmware version {}...", selected.version);
    let payloads = download_firmware_payloads(&host_identifier, selected).await?;
    Ok(ResolvedFirmware::Payloads(payloads))
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

pub async fn update_remote_host(firmware: Option<PathBuf>, force: bool) -> Result<(), Error> {
    let hosts = EP01UpdateTarget::attached().await;

    // Warn about unresponsive devices (nil identifier means HostInfo query failed)
    let unresponsive: Vec<&EP01UpdateTarget> = hosts
        .iter()
        .filter(|h| h.info().identifier == Identifier::nil())
        .collect();

    if !unresponsive.is_empty() && !force {
        println!("Warning: The following device(s) did not respond to host identification:");
        for host in &unresponsive {
            let mac = host.mac_address().unwrap_or("unknown");
            println!("  - MAC address: {}", mac);
        }
        println!("Verify only EP01 devices are attached to this computer before force updating.");
        print!("Continue? [y/N]: ");
        io::stdout()
            .flush()
            .map_err(|e| Error::Debug(format!("Failed to flush stdout: {e}")))?;

        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|e| Error::Debug(format!("Failed to read confirmation: {e}")))?;

        let trimmed = line.trim();
        if !trimmed.eq_ignore_ascii_case("y") {
            println!("Update aborted.");
            return Ok(());
        }
    }

    let selected_host = select_update_target(hosts)?;
    let host_info = selected_host.info.clone();
    let host_identifier = host_info.identifier;
    let current_version = &host_info.version;
    println!(
        "Updating host {} (current version {}).",
        host_identifier, current_version
    );

    let resolved = match resolve_firmware(host_identifier, current_version, firmware).await {
        Ok(r) => r,
        Err(e) => {
            log::debug!("Firmware resolution failed: {:?}", e);
            println!("Encountered error fetching firmware.");
            std::process::exit(1);
        }
    };

    match &resolved {
        ResolvedFirmware::LocalFile(path) => {
            println!("Using firmware image {}.", path.display());
            println!("Flashing selected device...");
            selected_host.flash_firmware_image(path)?;
        }
        ResolvedFirmware::Payloads(payloads) => {
            println!("Flashing {} payload(s)...", payloads.len());
            selected_host.flash_payloads(payloads)?;
        }
    }

    println!("Flash complete. Verifying update...");
    selected_host.verify_updated_host().await?;
    println!("Update verified.");

    Ok(())
}
