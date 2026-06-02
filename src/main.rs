use clap::{Parser, Subcommand};
use enody::{
    environment::{DiscoveryEnvironment, Environment},
    message::{Network, Token, WifiAuth},
    runtime::remote::RemoteRuntime,
    token_store::TokenStore,
    usb::UsbEnvironment,
    wifi::{WifiConnection, WifiDiscoveredDevice, WifiEnvironment},
    Identifier,
};
use std::{
    collections::{HashMap, HashSet},
    env,
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

macro_rules! vprintln {
    ($verbose:expr, $($arg:tt)*) => {
        if $verbose {
            println!($($arg)*);
        }
    };
}

const WIFI_TOKEN_VERIFY_ATTEMPTS: usize = 8;
const WIFI_TOKEN_VERIFY_RETRY_DELAY: Duration = Duration::from_millis(500);

struct DiscoveredRuntimes {
    _usb_environment: Option<UsbEnvironment>,
    _wifi_environment: Option<WifiEnvironment>,
    runtimes: Vec<RemoteRuntime>,
}

async fn collect_host_ids(runtimes: &[RemoteRuntime]) -> HashSet<Identifier> {
    let mut host_ids = HashSet::new();
    for runtime in runtimes {
        if let Ok(host) = runtime.host().await {
            host_ids.insert(host.identifier());
        }
    }
    host_ids
}

async fn discover_runtimes() -> Result<DiscoveredRuntimes, enody::Error> {
    let usb_environment = env::var_os("ENODY_DISABLE_USB")
        .filter(|value| !value.is_empty())
        .map(|_| None)
        .unwrap_or_else(|| Some(UsbEnvironment::new()));
    let mut runtimes = usb_environment
        .as_ref()
        .map(UsbEnvironment::runtimes)
        .unwrap_or_default();
    let usb_host_ids = collect_host_ids(&runtimes).await;
    let wifi_environment = match WifiEnvironment::with_excluded_host_ids(
        TokenStore::load()?.into_tokens(),
        usb_host_ids,
    )
    .await
    {
        Ok(environment) => {
            runtimes.extend(environment.runtimes());
            Some(environment)
        }
        Err(error) => {
            log::debug!("WiFi runtime discovery failed: {:?}", error);
            None
        }
    };
    Ok(DiscoveredRuntimes {
        _usb_environment: usb_environment,
        _wifi_environment: wifi_environment,
        runtimes,
    })
}

#[derive(Parser)]
#[command(name = "enody")]
#[command(about = "Enody Host SDK CLI", long_about = None)]
struct EnodyCLI {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all attached Enody devices
    List,

    /// Display detailed information about all attached devices
    Info,

    /// Monitor log output from all attached devices
    Monitor,

    /// Monitor USB hotplug events (arrived/left)
    Hotplug,

    /// Set all fixtures to a blackbody configuration
    SetBlackbody {
        /// Correlated color temperature in Kelvin
        cct: f32,

        /// Target relative flux (0.0 to 1.0, default: 0.5)
        #[arg(short, long, default_value_t = 0.5)]
        flux: f32,
    },

    /// Set all fixtures to a chromaticity configuration
    SetChromaticity {
        /// CIE 1931 x coordinate
        x: f32,

        /// CIE 1931 y coordinate
        y: f32,

        /// Target relative flux (0.0 to 1.0, default: 0.5)
        #[arg(short, long, default_value_t = 0.5)]
        flux: f32,
    },

    /// Strobe all fixtures between off and a target flux at a given CCT
    Strobe {
        /// Correlated color temperature in Kelvin
        cct: f32,

        /// Target relative flux (0.0 to 1.0, default: 0.5)
        #[arg(short, long, default_value_t = 0.5)]
        flux: f32,

        /// Duration in seconds (default: 1.0)
        #[arg(short, long, default_value_t = 1.0)]
        duration: f32,

        /// Target framerate in fps (default: 60, max: 240)
        #[arg(short, long, default_value_t = 60.0)]
        rate: f32,
    },

    /// Linear fade between two blackbody CCT/flux settings
    Fade {
        /// Starting CCT in Kelvin (default: 3200)
        #[arg(long, default_value_t = 3200.0)]
        from_cct: f32,

        /// Ending CCT in Kelvin (default: 1000)
        #[arg(long, default_value_t = 1000.0)]
        to_cct: f32,

        /// Starting relative flux (default: 0.5)
        #[arg(long, default_value_t = 0.5)]
        from_flux: f32,

        /// Ending relative flux (default: 0.5)
        #[arg(long, default_value_t = 0.5)]
        to_flux: f32,

        /// Duration in seconds (default: 1.0)
        #[arg(short, long, default_value_t = 1.0)]
        duration: f32,

        /// Target framerate in fps (default: 60, max: 240)
        #[arg(short, long, default_value_t = 60.0)]
        rate: f32,
    },

    /// Scan each emitter individually, activating one at a time
    Scan {
        /// Relative flux for each emitter (0.0 to 1.0, default: 0.5)
        #[arg(short, long, default_value_t = 0.5)]
        flux: f32,

        /// Duration each emitter is held on, in milliseconds (default: 200)
        #[arg(short, long, default_value_t = 200)]
        duration: u64,
    },

    /// Download spectral data from all emitters and save as JSON
    DownloadSpectralData {
        /// Output file path
        #[arg(short, long, default_value = "spectral-data.json")]
        output: String,
    },

    /// Update selected device to newest firmware
    Update {
        /// Path to an offline firmware image (.bin)
        #[arg(short, long, value_name = "FILE")]
        firmware: Option<PathBuf>,
        /// Force update even if device does not respond to host identification
        #[arg(long)]
        force: bool,
    },
    SettingGet {
        key: String,
    },
    SettingSet {
        key: String,
        value: String,
    },
    SettingDelete {
        key: String,
    },
    /// Guided WiFi scan, join, and token setup through the trusted USB connection
    WifiSetup,

    /// Discover an EP01 over mDNS, then generate and save a WiFi token with physical approval
    WifiGenerateToken {
        /// mDNS discovery timeout in milliseconds
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,
    },
}

#[tokio::main]
async fn main() -> Result<(), enody::Error> {
    env_logger::Builder::from_default_env()
        .format_timestamp_millis()
        .init();

    let cli = EnodyCLI::parse();
    match cli.command {
        Commands::List => list_devices().await?,
        Commands::Info => info_devices().await?,
        Commands::Monitor => monitor_devices().await?,
        Commands::Hotplug => hotplug_monitor().await?,
        Commands::SetBlackbody { cct, flux } => set_blackbody(cct, flux, cli.verbose).await?,
        Commands::SetChromaticity { x, y, flux } => {
            set_chromaticity(x, y, flux, cli.verbose).await?
        }
        Commands::Strobe {
            cct,
            flux,
            duration,
            rate,
        } => strobe(cct, flux, duration, rate, cli.verbose).await?,
        Commands::Fade {
            from_cct,
            to_cct,
            from_flux,
            to_flux,
            duration,
            rate,
        } => {
            fade(
                from_cct,
                to_cct,
                from_flux,
                to_flux,
                duration,
                rate,
                cli.verbose,
            )
            .await?
        }
        Commands::Scan { flux, duration } => scan(flux, duration).await?,
        Commands::DownloadSpectralData { output } => download_spectral_data(&output).await?,
        Commands::Update { firmware, force } => {
            enody::update::update_remote_host(firmware, force).await?
        }
        Commands::SettingGet { key } => setting_get(&key).await?,
        Commands::SettingSet { key, value } => setting_set(&key, &value, cli.verbose).await?,
        Commands::SettingDelete { key } => setting_delete(&key, cli.verbose).await?,
        Commands::WifiSetup => wifi_setup().await?,
        Commands::WifiGenerateToken { timeout_ms } => {
            wifi_generate_token_from_mdns(Duration::from_millis(timeout_ms)).await?
        }
    }

    Ok(())
}

async fn list_devices() -> Result<(), enody::Error> {
    let discovered = discover_runtimes().await?;
    if discovered.runtimes.is_empty() {
        println!("No Enody devices found.");
    } else {
        for runtime in &discovered.runtimes {
            let Ok(host) = runtime.host().await else {
                println!("Failed to query host.");
                continue;
            };
            println!("Device {}", host.identifier());
            println!("\tVersion: {}", host.version());
        }
    }

    Ok(())
}

async fn info_devices() -> Result<(), enody::Error> {
    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        println!("No Enody devices found.");
        return Ok(());
    }

    for (device_idx, runtime) in runtimes.iter().enumerate() {
        if device_idx > 0 {
            println!();
        }

        println!("══════════════════════════════════════════════════════════════");
        println!("Device {}", device_idx + 1);
        println!("══════════════════════════════════════════════════════════════");

        // Query host information
        let Ok(host) = runtime.host().await else {
            println!("  Failed to query host");
            continue;
        };

        println!();
        println!("Host");
        println!("────────────────────────────────────────────────────────────────");
        println!("  Identifier: {}", host.identifier());
        println!("  Version:    {}", host.version());

        // Discover fixtures and display their info
        let Ok(fixtures) = host.fixtures().await else {
            println!("  Failed to discover fixtures");
            continue;
        };
        println!("  Fixtures:   {}", fixtures.len());

        for (fixture_idx, fixture) in fixtures.iter().enumerate() {
            println!();
            println!("Fixture {}", fixture_idx + 1);
            println!("────────────────────────────────────────────────────────────────");
            println!("  Identifier: {}", fixture.identifier());

            // Discover sources for this fixture
            let sources = fixture.sources().await;
            let Ok(sources) = sources else {
                println!(
                    "  Sources:    (failed to discover: {:?})",
                    sources.err().unwrap()
                );
                continue;
            };
            println!("  Sources:    {}", sources.len());

            for (source_idx, source) in sources.iter().enumerate() {
                println!();
                println!("  Source {}", source_idx + 1);
                println!("  ──────────────────────────────────────────────────────────");
                println!("    Identifier: {}", source.identifier());

                match source.emitter_count().await {
                    Ok(count) => println!("    Emitters:   {}", count),
                    Err(e) => println!("    Emitters:   (failed to query: {:?})", e),
                }
            }
        }
    }

    Ok(())
}

async fn monitor_devices() -> Result<(), enody::Error> {
    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        println!("No Enody devices found.");
        return Ok(());
    }

    println!(
        "Monitoring {} device(s). Press Ctrl+C to exit.",
        runtimes.len()
    );

    // Enable logging on all runtimes
    for runtime in runtimes {
        runtime.enable_logging();
    }

    // Wait for Ctrl+C
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");

    println!("\nShutting down...");
    Ok(())
}

async fn hotplug_monitor() -> Result<(), enody::Error> {
    use enody::environment::EnvironmentRuntimeEvent;

    let mut usb_environment = UsbEnvironment::new();
    usb_environment.start_discovery().await?;

    let mut initial_runtimes = usb_environment.runtimes();
    let mut usb_connection_host_ids = HashMap::new();
    let mut usb_host_ids = HashSet::new();
    for runtime in &initial_runtimes {
        if let Ok(host) = runtime.host().await {
            usb_connection_host_ids.insert(runtime.connection().identifier(), host.identifier());
            usb_host_ids.insert(host.identifier());
        }
    }
    let mut wifi_environment = match WifiEnvironment::with_excluded_host_ids(
        TokenStore::load()?.into_tokens(),
        usb_host_ids,
    )
    .await
    {
        Ok(mut environment) => {
            environment.start_discovery().await?;
            initial_runtimes.extend(environment.runtimes());
            Some(environment)
        }
        Err(error) => {
            log::debug!("WiFi runtime discovery failed: {:?}", error);
            None
        }
    };
    println!("Hotplug monitor active. Press Ctrl+C to exit.");
    println!("Currently connected: {}", initial_runtimes.len());
    for runtime in &initial_runtimes {
        if let Ok(host) = runtime.host().await {
            println!("  {}", host.identifier());
        }
    }

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\nStopping hotplug monitor...");
                break;
            }
            event = usb_environment.next_runtime_event() => {
                match event? {
                    EnvironmentRuntimeEvent::Arrived(runtime) => {
                        let connection_id = runtime.connection().identifier();
                        match runtime.host().await {
                            Ok(host) => {
                                let host_id = host.identifier();
                                usb_connection_host_ids.insert(connection_id, host_id);
                                if let Some(environment) = wifi_environment.as_ref() {
                                    environment.exclude_host_id(host_id).await;
                                }
                                println!("USB arrived: {}", host_id);
                            }
                            Err(error) => {
                                log::warn!("Failed to query arrived USB host: {:?}", error);
                                println!("USB arrived: {}", connection_id);
                            }
                        }
                    }
                    EnvironmentRuntimeEvent::Left(runtime) => {
                        let connection_id = runtime.connection().identifier();
                        match usb_connection_host_ids.remove(&connection_id) {
                            Some(host_id) => {
                                if let Some(environment) = wifi_environment.as_ref() {
                                    environment.remove_excluded_host_id(host_id);
                                }
                                println!("USB left: {}", host_id);
                            }
                            None => {
                                println!("USB left: {}", connection_id);
                            }
                        }
                    }
                }
            }
            event = async {
                match wifi_environment.as_ref() {
                    Some(environment) => environment.next_runtime_event().await,
                    None => std::future::pending().await,
                }
            } => {
                match event? {
                    EnvironmentRuntimeEvent::Arrived(runtime) => {
                        println!("WiFi arrived: {}", runtime.connection().identifier());
                    }
                    EnvironmentRuntimeEvent::Left(runtime) => {
                        println!("WiFi left: {}", runtime.connection().identifier());
                    }
                }
            }
        }
    }

    usb_environment.stop_discovery().await?;
    if let Some(environment) = wifi_environment.as_mut() {
        environment.stop_discovery().await?;
    }
    Ok(())
}

async fn set_blackbody(cct: f32, flux: f32, verbose: bool) -> Result<(), enody::Error> {
    use enody::message::{Configuration, Flux};

    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    let config = Configuration::Blackbody(cct);
    let target_flux = Flux::Relative(flux);

    for runtime in runtimes {
        let Ok(host) = runtime.host().await else {
            vprintln!(verbose, "Failed to query host");
            continue;
        };

        let Ok(fixtures) = host.fixtures().await else {
            vprintln!(verbose, "Failed to discover fixtures");
            continue;
        };

        for fixture in &fixtures {
            match fixture.display(config.clone(), target_flux.clone()).await {
                Ok((result_config, result_flux)) => {
                    vprintln!(
                        verbose,
                        "Fixture {} set to {:?} at {:?}",
                        fixture.identifier(),
                        result_config,
                        result_flux
                    );
                }
                Err(e) => {
                    vprintln!(
                        verbose,
                        "Failed to set fixture {}: {:?}",
                        fixture.identifier(),
                        e
                    );
                }
            }
        }
    }

    Ok(())
}

async fn set_chromaticity(x: f32, y: f32, flux: f32, verbose: bool) -> Result<(), enody::Error> {
    use enody::message::{Chromaticity, Configuration, Flux};

    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    let config = Configuration::Chromatic(Chromaticity { x, y });
    let target_flux = Flux::Relative(flux);

    for runtime in runtimes {
        let Ok(host) = runtime.host().await else {
            vprintln!(verbose, "Failed to query host");
            continue;
        };

        let Ok(fixtures) = host.fixtures().await else {
            vprintln!(verbose, "Failed to discover fixtures");
            continue;
        };

        for fixture in &fixtures {
            match fixture.display(config.clone(), target_flux.clone()).await {
                Ok((result_config, result_flux)) => {
                    vprintln!(
                        verbose,
                        "Fixture {} set to {:?} at {:?}",
                        fixture.identifier(),
                        result_config,
                        result_flux
                    );
                }
                Err(e) => {
                    vprintln!(
                        verbose,
                        "Failed to set fixture {}: {:?}",
                        fixture.identifier(),
                        e
                    );
                }
            }
        }
    }

    Ok(())
}

async fn scan(flux: f32, duration_ms: u64) -> Result<(), enody::Error> {
    use enody::message::{Configuration, Flux};
    use std::time::Duration;

    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        println!("No Enody devices found.");
        return Ok(());
    }

    // Collect all (fixture, emitter_label, emitter) tuples across all devices
    let mut scan_entries = Vec::new();

    for runtime in runtimes {
        let Ok(host) = runtime.host().await else {
            println!("Failed to query host");
            continue;
        };
        println!("Host: {} (v{})", host.identifier(), host.version());

        let Ok(fixtures) = host.fixtures().await else {
            println!("  Failed to discover fixtures");
            continue;
        };

        for (fi, fixture) in fixtures.into_iter().enumerate() {
            println!("Fixture {}: {}", fi, fixture.identifier());

            let Ok(sources) = fixture.sources().await else {
                println!("  Failed to discover sources");
                continue;
            };

            for (si, source) in sources.iter().enumerate() {
                println!("  Source {}: {}", si, source.identifier());

                let Ok(emitters) = source.emitters().await else {
                    println!("    Failed to discover emitters");
                    continue;
                };

                for (ei, emitter) in emitters.into_iter().enumerate() {
                    let label = format!("F{}S{}E{}", fi, si, ei);
                    println!("    Emitter {} ({}): {}", ei, label, emitter.identifier());
                    scan_entries.push((fixture.clone(), label, emitter));
                }
            }
        }
    }

    println!(
        "\nScanning {} emitters (flux {:.2} / {}ms each)...\n",
        scan_entries.len(),
        flux,
        duration_ms,
    );

    let flux_on = Flux::Relative(flux);
    let flux_off = Flux::Relative(0.0);

    for (fixture, label, emitter) in &scan_entries {
        print!("  {} ({})... ", label, emitter.identifier());

        emitter.set_flux(flux_on.clone()).await?;
        fixture
            .display(Configuration::Manual, Flux::Relative(0.5))
            .await?;

        tokio::time::sleep(Duration::from_millis(duration_ms)).await;

        emitter.set_flux(flux_off.clone()).await?;
        fixture
            .display(Configuration::Manual, Flux::Relative(0.5))
            .await?;

        println!("done");
    }

    println!("\nScan complete.");
    Ok(())
}

async fn download_spectral_data(output_path: &str) -> Result<(), enody::Error> {
    use enody::spectral::{
        EmitterSpectralData, FixtureSpectralData, HostSpectralData, SourceSpectralData,
    };

    if std::path::Path::new(output_path).exists() {
        return Err(enody::Error::Debug(format!(
            "Output file already exists: {}",
            output_path
        )));
    }

    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        println!("No Enody devices found.");
        return Ok(());
    }

    // Use the first runtime
    let runtime = &runtimes[0];
    let Ok(host) = runtime.host().await else {
        println!("Failed to query host");
        return Ok(());
    };
    println!("Host: {} (v{})", host.identifier(), host.version());

    let Ok(fixtures) = host.fixtures().await else {
        println!("Failed to discover fixtures");
        return Ok(());
    };
    println!("Fixtures: {}", fixtures.len());

    let mut fixture_outputs = Vec::new();

    for (fi, fixture) in fixtures.iter().enumerate() {
        println!("  Fixture {}: {}", fi, fixture.identifier());

        let Ok(sources) = fixture.sources().await else {
            println!("    Failed to discover sources");
            continue;
        };
        println!("    Sources: {}", sources.len());

        let mut source_outputs = Vec::new();

        for (si, source) in sources.iter().enumerate() {
            println!("    Source {}: {}", si, source.identifier());

            let Ok(emitters) = source.emitters().await else {
                println!("      Failed to discover emitters");
                continue;
            };
            println!("      Emitters: {}", emitters.len());

            let mut emitter_outputs = Vec::new();

            for (ei, emitter) in emitters.iter().enumerate() {
                println!("      Emitter {}: {}", ei, emitter.identifier());

                let spectral_data = emitter.spectral_data().await?;
                println!("        Samples: {}", spectral_data.samples().len());

                emitter_outputs.push(EmitterSpectralData {
                    identifier: emitter.identifier(),
                    spectral_data,
                });
            }

            source_outputs.push(SourceSpectralData {
                identifier: source.identifier(),
                emitters: emitter_outputs,
            });
        }

        fixture_outputs.push(FixtureSpectralData {
            identifier: fixture.identifier(),
            sources: source_outputs,
        });
    }

    let output = HostSpectralData {
        host: host.info().clone(),
        fixtures: fixture_outputs,
    };

    let json = serde_json::to_string_pretty(&output)
        .map_err(|e| enody::Error::Debug(format!("JSON serialization failed: {}", e)))?;

    std::fs::write(output_path, &json)
        .map_err(|e| enody::Error::Debug(format!("Failed to write {}: {}", output_path, e)))?;

    println!("Spectral data written to {}", output_path);
    Ok(())
}

async fn strobe(
    cct: f32,
    flux: f32,
    duration: f32,
    rate: f32,
    verbose: bool,
) -> Result<(), enody::Error> {
    use enody::message::{Configuration, Flux};
    use std::time::Duration;

    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    let config = Configuration::Blackbody(cct);
    let flux_on = Flux::Relative(flux);
    let flux_off = Flux::Relative(0.0);
    let frame_duration = Duration::from_secs_f32(1.0 / rate.min(240.0));
    let total_frames = (duration * rate.min(240.0)) as u32;

    // Collect all fixtures across all runtimes
    let mut fixtures = Vec::new();
    for (index, runtime) in runtimes.iter().enumerate() {
        let Ok(host) = runtime.host().await else {
            vprintln!(verbose, "Failed to query host on runtime {}", index + 1);
            continue;
        };

        let Ok(f) = host.fixtures().await else {
            vprintln!(
                verbose,
                "Failed to discover fixtures on runtime {}",
                index + 1
            );
            continue;
        };
        fixtures.extend(f);
    }

    if fixtures.is_empty() {
        vprintln!(verbose, "No fixtures found.");
        return Ok(());
    }

    let mut interval = tokio::time::interval(frame_duration);
    let mut on = true;
    let mut cycles: u32 = 0;
    for _ in 0..total_frames {
        interval.tick().await;
        let target = if on { &flux_on } else { &flux_off };
        for fixture in &fixtures {
            let _ = fixture.display(config.clone(), target.clone()).await;
        }
        on = !on;
        cycles += 1;
    }

    // Ensure fixtures are left off
    for fixture in &fixtures {
        let _ = fixture.display(config.clone(), flux_off.clone()).await;
    }

    vprintln!(verbose, "{} cycles in {:.2}s", cycles, duration);

    Ok(())
}

async fn fade(
    from_cct: f32,
    to_cct: f32,
    from_flux: f32,
    to_flux: f32,
    duration: f32,
    rate: f32,
    verbose: bool,
) -> Result<(), enody::Error> {
    use enody::message::{Configuration, Flux};
    use std::time::Duration;

    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    let capped_rate = rate.min(240.0);
    let total_frames = (duration * capped_rate) as u32;
    let frame_duration = Duration::from_secs_f32(1.0 / capped_rate);

    let mut fixtures = Vec::new();
    for (index, runtime) in runtimes.iter().enumerate() {
        let Ok(host) = runtime.host().await else {
            vprintln!(verbose, "Failed to query host on runtime {}", index + 1);
            continue;
        };

        let Ok(f) = host.fixtures().await else {
            vprintln!(
                verbose,
                "Failed to discover fixtures on runtime {}",
                index + 1
            );
            continue;
        };
        fixtures.extend(f);
    }

    if fixtures.is_empty() {
        vprintln!(verbose, "No fixtures found.");
        return Ok(());
    }

    let mut interval = tokio::time::interval(frame_duration);
    for frame in 0..=total_frames {
        interval.tick().await;
        let t = if total_frames == 0 {
            1.0
        } else {
            frame as f32 / total_frames as f32
        };
        let cct = from_cct + (to_cct - from_cct) * t;
        let flux = from_flux + (to_flux - from_flux) * t;
        let config = Configuration::Blackbody(cct);
        let target_flux = Flux::Relative(flux);

        for fixture in &fixtures {
            let _ = fixture.display(config.clone(), target_flux.clone()).await;
        }
    }

    vprintln!(
        verbose,
        "Fade complete: {} frames in {:.2}s",
        total_frames + 1,
        duration
    );

    Ok(())
}

async fn setting_get(key: &str) -> Result<(), enody::Error> {
    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        println!("No Enody devices found.");
        return Ok(());
    }

    for runtime in runtimes {
        let Ok(value) = runtime.setting_get::<Vec<u8>>(key).await else {
            println!("Failed to get setting '{}'", key);
            continue;
        };
        println!("{}: {:?}", key, value);
    }

    Ok(())
}

// Action commands — verbose-gated (like set_blackbody / set_chromaticity):
async fn setting_set(key: &str, value: &str, verbose: bool) -> Result<(), enody::Error> {
    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    let parsed: Vec<u8> = value.as_bytes().to_vec();

    for runtime in runtimes {
        match runtime.setting_set(key, parsed.clone()).await {
            Ok(()) => vprintln!(verbose, "Set '{}' = {:?}", key, parsed),
            Err(e) => vprintln!(verbose, "Failed to set '{}': {:?}", key, e),
        }
    }

    Ok(())
}

async fn setting_delete(key: &str, verbose: bool) -> Result<(), enody::Error> {
    let discovered = discover_runtimes().await?;
    let runtimes = &discovered.runtimes;

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    for runtime in runtimes {
        match runtime.setting_delete(key).await {
            Ok(()) => vprintln!(verbose, "Deleted '{}'", key),
            Err(e) => vprintln!(verbose, "Failed to delete '{}': {:?}", key, e),
        }
    }

    Ok(())
}

struct SelectedRuntime {
    _environment: UsbEnvironment,
    runtime: enody::runtime::remote::RemoteRuntime,
}

async fn first_usb_runtime() -> Result<SelectedRuntime, enody::Error> {
    let environment = UsbEnvironment::new();
    let runtimes = environment.runtimes();
    let runtime = runtimes
        .into_iter()
        .next()
        .ok_or(enody::Error::InsufficientData)?;
    Ok(SelectedRuntime {
        _environment: environment,
        runtime,
    })
}

fn print_wifi_networks(networks: &[Network]) {
    if networks.is_empty() {
        println!("No WiFi networks found.");
        return;
    }

    println!("WiFi networks:");
    for (index, network) in networks.iter().enumerate() {
        let Network::Wifi(network) = network;
        let auth = match network.auth.as_ref().unwrap_or(&WifiAuth::Unknown) {
            WifiAuth::Open => "open",
            WifiAuth::Secured => "secured",
            WifiAuth::Unknown => "unknown",
        };
        let ssid = network
            .ssid
            .as_ref()
            .map(|ssid| ssid.as_str())
            .unwrap_or("<hidden>");
        let rssi = network
            .rssi
            .map(|rssi| rssi.to_string())
            .unwrap_or_else(|| "-".to_string());
        let channel = network
            .channel
            .map(|channel| channel.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:>2}. {:<32} rssi={:>4} channel={:<2} auth={}",
            index + 1,
            ssid,
            rssi,
            channel,
            auth
        );
    }
}

async fn wifi_setup() -> Result<(), enody::Error> {
    let selected = first_usb_runtime().await?;
    let host = selected.runtime.host().await?;

    println!("Scanning for WiFi networks...");
    let networks = host.wifi_scan().await?;
    let ssid = prompt_wifi_ssid(&networks)?;
    let password = prompt_wifi_password()?;

    println!("Joining WiFi network {:?}...", ssid);
    host.wifi_join(&ssid, &password).await?;
    println!("Joined WiFi network {:?}.", ssid);

    println!("Generating authentication token...");
    generate_token_for_runtime(&selected.runtime).await
}

async fn wifi_generate_token_from_mdns(timeout: Duration) -> Result<(), enody::Error> {
    println!("Searching for EP01s over mDNS...");
    let device = select_wifi_token_generation_device(timeout).await?;
    let endpoint = device.endpoint().ok_or(enody::Error::Argument)?;

    println!("Generating WiFi token from {}.", endpoint);
    println!("When EP01 starts pulsing, approve or cancel the request on the device.");

    let token = WifiConnection::generate_token_from_discovered_device_with_approval(
        &device,
        |instruction| {
            println!("Approval required: {}", instruction);
        },
    )
    .await?;

    println!("Token generated. Verifying authenticated WiFi connection...");
    let host_id = verify_wifi_token_from_discovered_device(&token, &device).await?;

    let path = TokenStore::save_token(&token)?;
    println!("Verified WiFi token for device {}.", host_id);
    println!("Saved token to {}", path.display());

    Ok(())
}

async fn verify_wifi_token_from_discovered_device(
    token: &Token,
    device: &WifiDiscoveredDevice,
) -> Result<Identifier, enody::Error> {
    for attempt in 1..=WIFI_TOKEN_VERIFY_ATTEMPTS {
        let runtime = WifiConnection::runtime_from_discovered_device(token, device)?;
        match verify_wifi_runtime(&runtime).await {
            Ok(host_id) => return Ok(host_id),
            Err(error) if attempt < WIFI_TOKEN_VERIFY_ATTEMPTS => {
                log::debug!(
                    "WiFi token verification attempt {}/{} failed: {:?}; retrying in {:?}",
                    attempt,
                    WIFI_TOKEN_VERIFY_ATTEMPTS,
                    error,
                    WIFI_TOKEN_VERIFY_RETRY_DELAY
                );
                tokio::time::sleep(WIFI_TOKEN_VERIFY_RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }

    Err(enody::Error::Timeout)
}

async fn verify_wifi_runtime(runtime: &RemoteRuntime) -> Result<Identifier, enody::Error> {
    runtime.connect().await?;
    let host = runtime.host().await;
    let disconnect = runtime.disconnect().await;
    let host = host?;
    disconnect?;
    Ok(host.identifier())
}

async fn select_wifi_token_generation_device(
    timeout: Duration,
) -> Result<WifiDiscoveredDevice, enody::Error> {
    let devices = WifiConnection::discover_token_generation_devices(timeout).await?;
    match devices.len() {
        0 => {
            println!("No EP01s found for WiFi token generation.");
            Err(enody::Error::InsufficientData)
        }
        1 => {
            let device = devices
                .into_iter()
                .next()
                .ok_or(enody::Error::InsufficientData)?;
            println!(
                "Found EP01 {} at {}.",
                device
                    .host_id
                    .map(|host_id| host_id.to_string())
                    .unwrap_or_else(|| "unknown host".to_string()),
                device
                    .endpoint()
                    .unwrap_or_else(|| "unknown endpoint".to_string())
            );
            Ok(device)
        }
        _ => prompt_wifi_token_generation_device(devices),
    }
}

fn prompt_wifi_token_generation_device(
    devices: Vec<WifiDiscoveredDevice>,
) -> Result<WifiDiscoveredDevice, enody::Error> {
    print_wifi_token_generation_devices(&devices);

    loop {
        let input = prompt_line("Pick an EP01 number: ")?;
        let selected = match input.trim().parse::<usize>() {
            Ok(selected) if (1..=devices.len()).contains(&selected) => selected,
            _ => {
                println!("Enter a number from 1 to {}.", devices.len());
                continue;
            }
        };

        return Ok(devices[selected - 1].clone());
    }
}

fn print_wifi_token_generation_devices(devices: &[WifiDiscoveredDevice]) {
    println!("Available EP01s:");
    for (index, device) in devices.iter().enumerate() {
        let host_id = device
            .host_id
            .map(|host_id| host_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let endpoint = device.endpoint().unwrap_or_else(|| "unknown".to_string());
        let firmware = device
            .firmware_version
            .as_deref()
            .unwrap_or("unknown firmware");
        println!(
            "{:>2}. host={} endpoint={} firmware={}",
            index + 1,
            host_id,
            endpoint,
            firmware
        );
    }
}

fn prompt_wifi_ssid(networks: &[Network]) -> Result<String, enody::Error> {
    print_wifi_networks(networks);
    if networks.is_empty() {
        return prompt_manual_ssid();
    }

    loop {
        let input = prompt_line("Pick a network number (Enter to type SSID): ")?;
        let input = input.trim();
        if input.is_empty() {
            return prompt_manual_ssid();
        }

        let selected = match input.parse::<usize>() {
            Ok(selected) if (1..=networks.len()).contains(&selected) => selected,
            _ => {
                println!(
                    "Enter a number from 1 to {} or press Enter to type an SSID.",
                    networks.len()
                );
                continue;
            }
        };

        let Network::Wifi(network) = &networks[selected - 1];
        if let Some(ssid) = network.ssid.as_ref() {
            return Ok(ssid.to_string());
        }

        println!("Selected network has a hidden SSID.");
        return prompt_manual_ssid();
    }
}

fn prompt_manual_ssid() -> Result<String, enody::Error> {
    loop {
        let ssid = prompt_line("SSID: ")?;
        if !ssid.is_empty() {
            return Ok(ssid);
        }
        println!("SSID cannot be empty.");
    }
}

fn prompt_line(prompt: &str) -> Result<String, enody::Error> {
    print!("{}", prompt);
    io::stdout()
        .flush()
        .map_err(|error| enody::Error::Debug(error.to_string()))?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|error| enody::Error::Debug(error.to_string()))?;
    while input.ends_with('\n') || input.ends_with('\r') {
        input.pop();
    }
    Ok(input)
}

fn prompt_wifi_password() -> Result<String, enody::Error> {
    dialoguer::Password::new()
        .with_prompt("Password")
        .allow_empty_password(true)
        .interact()
        .map_err(|error| enody::Error::Debug(error.to_string()))
}

async fn generate_token_for_runtime(
    runtime: &enody::runtime::remote::RemoteRuntime,
) -> Result<(), enody::Error> {
    let token = runtime.generate_token().await?;
    let path = TokenStore::save_token(&token)?;
    println!("Saved token to {}", path.display());
    Ok(())
}
