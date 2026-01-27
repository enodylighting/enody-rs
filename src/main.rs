use clap::{Parser, Subcommand};
use enody::{environment::Environment, runtime::usb::USBEnvironment};

#[derive(Parser)]
#[command(name = "enody")]
#[command(about = "Enody Host SDK CLI", long_about = None)]
struct EnodyCLI {
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

            // Query source count for this fixture
            match fixture.source_count().await {
                Ok(count) => println!("  Sources:    {}", count),
                Err(e) => println!("  Sources:    (failed to query: {:?})", e),
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

async fn update_remote_host() -> Result<(), Box<enody::Error>> {
    Ok(())
}
