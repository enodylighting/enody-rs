use clap::{Parser, Subcommand};
use enody::remote::RemoteHost;
use log;

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
    let devices = RemoteHost::attached();
    match devices {
        Ok(devices) => {
            for device in devices {
                println!("Device {}", device.identifier().await?);
                println!("\tVersion: {}", device.version().await?);
            }
        }
        Err(e) => {
            log::error!("Failed to enumerate devices: {:?}", e);
        }
    }
    Ok(())
}

async fn update_remote_host() -> Result<(), Box<enody::Error>> {
    Ok(())
}