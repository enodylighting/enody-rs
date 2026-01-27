use clap::{Parser, Subcommand};
use enody::{environment::Environment, runtime::usb::USBEnvironment};

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
            match runtime.host().await {
                Ok(host) => {
                    println!("Device {}", host.identifier());
                    println!("\tVersion: {}", host.version());
                }
                Err(e) => {
                    println!("Failed to query host: {:?}", e);
                }
            }
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
        let host = match runtime.host().await {
            Ok(host) => host,
            Err(e) => {
                println!("  Failed to query host: {:?}", e);
                continue;
            }
        };

        println!();
        println!("Host");
        println!("────────────────────────────────────────────────────────────────");
        println!("  Identifier: {}", host.identifier());
        println!("  Version:    {}", host.version());

        // Discover fixtures and display their info
        let fixtures = match host.fixtures().await {
            Ok(fixtures) => fixtures,
            Err(e) => {
                println!("  Failed to discover fixtures: {:?}", e);
                continue;
            }
        };
        println!("  Fixtures:   {}", fixtures.len());

        for (fixture_idx, fixture) in fixtures.iter().enumerate() {
            println!();
            println!("Fixture {}", fixture_idx + 1);
            println!("────────────────────────────────────────────────────────────────");
            println!("  Identifier: {}", fixture.identifier());

            // Discover sources for this fixture
            let sources = match fixture.sources().await {
                Ok(sources) => sources,
                Err(e) => {
                    println!("  Sources:    (failed to discover: {:?})", e);
                    continue;
                }
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
        let host = match runtime.host().await {
            Ok(host) => host,
            Err(e) => {
                vprintln!(verbose, "Failed to query host: {:?}", e);
                continue;
            }
        };

        let fixtures = match host.fixtures().await {
            Ok(fixtures) => fixtures,
            Err(e) => {
                vprintln!(verbose, "Failed to discover fixtures: {:?}", e);
                continue;
            }
        };

        for fixture in &fixtures {
            match fixture.display(config.clone(), target_flux.clone()).await {
                Ok((result_config, result_flux)) => {
                    vprintln!(verbose, "Fixture {} set to {:?} at {:?}", fixture.identifier(), result_config, result_flux);
                }
                Err(e) => {
                    vprintln!(verbose, "Failed to set fixture {}: {:?}", fixture.identifier(), e);
                }
            }
        }
    }

    Ok(())
}

async fn update_remote_host() -> Result<(), Box<enody::Error>> {
    Ok(())
}
