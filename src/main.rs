use clap::{Parser, Subcommand};
use enody::{environment::Environment, host::Host, runtime::usb::USBEnvironment};

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

    /// Update selected device to newest firmware
    Update,
}

#[tokio::main]
async fn main() -> Result<(), Box<enody::Error>> {
    env_logger::init();

    let cli = EnodyCLI::parse();
    match cli.command {
        Commands::List => list_devices().await?,
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

async fn update_remote_host() -> Result<(), Box<enody::Error>> {
    Ok(())
}
