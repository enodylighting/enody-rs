use clap::{Parser, Subcommand};
use enody::{environment::Environment, usb::USBEnvironment};

macro_rules! vprintln {
    ($verbose:expr, $($arg:tt)*) => {
        if $verbose {
            println!($($arg)*);
        }
    };
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

    /// Update selected device to newest firmware
    Update,
}

#[tokio::main]
async fn main() -> Result<(), Box<enody::Error>> {
    env_logger::init();

    let cli = EnodyCLI::parse();
    match cli.command {
        Commands::List => list_devices().await?,
        Commands::Info => info_devices().await?,
        Commands::Monitor => monitor_devices().await?,
        Commands::SetBlackbody { cct, flux } => set_blackbody(cct, flux, cli.verbose).await?,
        Commands::SetChromaticity { x, y, flux } => set_chromaticity(x, y, flux, cli.verbose).await?,
        Commands::Strobe { cct, flux, duration, rate } => strobe(cct, flux, duration, rate, cli.verbose).await?,
        Commands::Fade { from_cct, to_cct, from_flux, to_flux, duration, rate } => fade(from_cct, to_cct, from_flux, to_flux, duration, rate, cli.verbose).await?,
        Commands::Update => update_remote_host().await?
    }

    Ok(())
}

async fn list_devices() -> Result<(), Box<enody::Error>> {
    // Create a USB environment - this automatically enumerates attached devices
    let environment = USBEnvironment::new();

    // Get runtimes and create hosts via RemoteRuntime
    let runtimes = environment.runtimes();
    if runtimes.is_empty() {
        println!("No Enody devices found.");
    } else {
        for runtime in runtimes {
            let Ok(host) = runtime.host().await else {
                println!("Failed to query host: {:?}", runtime.host().await.unwrap_err());
                continue;
            };
            println!("Device {}", host.identifier());
            println!("\tVersion: {}", host.version());
        }
    }

    Ok(())
}

async fn info_devices() -> Result<(), Box<enody::Error>> {
    let environment = USBEnvironment::new();
    let runtimes = environment.runtimes();

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
            println!("  Failed to query host: {:?}", runtime.host().await.unwrap_err());
            continue;
        };

        println!();
        println!("Host");
        println!("────────────────────────────────────────────────────────────────");
        println!("  Identifier: {}", host.identifier());
        println!("  Version:    {}", host.version());

        // Discover fixtures and display their info
        let Ok(fixtures) = host.fixtures().await else {
            println!("  Failed to discover fixtures: {:?}", host.fixtures().await.unwrap_err());
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
                println!("  Sources:    (failed to discover: {:?})", sources.err().unwrap());
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

async fn monitor_devices() -> Result<(), Box<enody::Error>> {
    let environment = USBEnvironment::new();
    let runtimes = environment.runtimes();

    if runtimes.is_empty() {
        println!("No Enody devices found.");
        return Ok(());
    }

    println!("Monitoring {} device(s). Press Ctrl+C to exit.", runtimes.len());

    // Enable logging on all runtimes
    for runtime in &runtimes {
        runtime.enable_logging();
    }

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await
        .expect("Failed to listen for Ctrl+C");

    println!("\nShutting down...");
    Ok(())
}

async fn set_blackbody(cct: f32, flux: f32, verbose: bool) -> Result<(), Box<enody::Error>> {
    use enody::message::{Configuration, Flux};

    let environment = USBEnvironment::new();
    let runtimes = environment.runtimes();

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    let config = Configuration::Blackbody(cct);
    let target_flux = Flux::Relative(flux);

    for runtime in &runtimes {
        let Ok(host) = runtime.host().await else {
            vprintln!(verbose, "Failed to query host: {:?}", runtime.host().await.unwrap_err());
            continue;
        };

        let Ok(fixtures) = host.fixtures().await else {
            vprintln!(verbose, "Failed to discover fixtures: {:?}", host.fixtures().await.unwrap_err());
            continue;
        };

        for fixture in &fixtures {
            let Ok((result_config, result_flux)) = fixture.display(config.clone(), target_flux.clone()).await else {
                vprintln!(verbose, "Failed to set fixture {}: {:?}", fixture.identifier(), fixture.display(config.clone(), target_flux.clone()).await.unwrap_err());
                continue;
            };
            vprintln!(verbose, "Fixture {} set to {:?} at {:?}", fixture.identifier(), result_config, result_flux);
        }
    }

    Ok(())
}

async fn set_chromaticity(x: f32, y: f32, flux: f32, verbose: bool) -> Result<(), Box<enody::Error>> {
    use enody::message::{Chromaticity, Configuration, Flux};

    let environment = USBEnvironment::new();
    let runtimes = environment.runtimes();

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    let config = Configuration::Chromatic(Chromaticity { x, y });
    let target_flux = Flux::Relative(flux);

    for runtime in &runtimes {
        let Ok(host) = runtime.host().await else {
            vprintln!(verbose, "Failed to query host: {:?}", runtime.host().await.unwrap_err());
            continue;
        };

        let Ok(fixtures) = host.fixtures().await else {
            vprintln!(verbose, "Failed to discover fixtures: {:?}", host.fixtures().await.unwrap_err());
            continue;
        };

        for fixture in &fixtures {
            let Ok((result_config, result_flux)) = fixture.display(config.clone(), target_flux.clone()).await else {
                vprintln!(verbose, "Failed to set fixture {}: {:?}", fixture.identifier(), fixture.display(config.clone(), target_flux.clone()).await.unwrap_err());
                continue;
            };
            vprintln!(verbose, "Fixture {} set to {:?} at {:?}", fixture.identifier(), result_config, result_flux);
        }
    }

    Ok(())
}

async fn strobe(cct: f32, flux: f32, duration: f32, rate: f32, verbose: bool) -> Result<(), Box<enody::Error>> {
    use enody::message::{Configuration, Flux};
    use std::time::Duration;

    let environment = USBEnvironment::new();
    let runtimes = environment.runtimes();

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
    for runtime in &runtimes {
        let Ok(host) = runtime.host().await else {
            vprintln!(verbose, "Failed to query host: {:?}", runtime.host().await.unwrap_err());
            continue;
        };

        let Ok(f) = host.fixtures().await else {
            vprintln!(verbose, "Failed to discover fixtures: {:?}", host.fixtures().await.unwrap_err());
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

async fn fade(from_cct: f32, to_cct: f32, from_flux: f32, to_flux: f32, duration: f32, rate: f32, verbose: bool) -> Result<(), Box<enody::Error>> {
    use enody::message::{Configuration, Flux};
    use std::time::Duration;

    let environment = USBEnvironment::new();
    let runtimes = environment.runtimes();

    if runtimes.is_empty() {
        vprintln!(verbose, "No Enody devices found.");
        return Ok(());
    }

    let capped_rate = rate.min(240.0);
    let total_frames = (duration * capped_rate) as u32;
    let frame_duration = Duration::from_secs_f32(1.0 / capped_rate);

    let mut fixtures = Vec::new();
    for runtime in &runtimes {
        let Ok(host) = runtime.host().await else {
            vprintln!(verbose, "Failed to query host: {:?}", runtime.host().await.unwrap_err());
            continue;
        };

        let Ok(f) = host.fixtures().await else {
            vprintln!(verbose, "Failed to discover fixtures: {:?}", host.fixtures().await.unwrap_err());
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
        let t = if total_frames == 0 { 1.0 } else { frame as f32 / total_frames as f32 };
        let cct = from_cct + (to_cct - from_cct) * t;
        let flux = from_flux + (to_flux - from_flux) * t;
        let config = Configuration::Blackbody(cct);
        let target_flux = Flux::Relative(flux);

        for fixture in &fixtures {
            let _ = fixture.display(config.clone(), target_flux.clone()).await;
        }
    }

    vprintln!(verbose, "Fade complete: {} frames in {:.2}s", total_frames + 1, duration);

    Ok(())
}

async fn update_remote_host() -> Result<(), Box<enody::Error>> {
    Ok(())
}
