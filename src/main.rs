use clap::{Parser, Subcommand};
use enody::remote::RemoteRuntime;
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
    let runtimes = RemoteRuntime::attached();
    match runtimes {
        Ok(mut runtimes) => {
            for runtime in runtimes.iter_mut() {
                let host = runtime.host();
                println!("Device {}", host.identifier().await?);
                println!("\tVersion: {}", host.version().await?);
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