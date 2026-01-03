use clap::{Parser, Subcommand};
use enody::remote::{USBDevice, USBRemoteRuntime};
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
}

#[tokio::main]
async fn main() -> Result<(), Box<enody::Error>> {
    env_logger::init();
    log::info!("enody rust CLI application");

    let cli = EnodyCLI::parse();

    match cli.command {
        Commands::List => list_devices()?,
    }

    Ok(())
}

fn list_devices() -> Result<(), Box<enody::Error>> {
    let devices = USBDevice::attached();
    match devices {
        Ok(devices) => {
            for device in devices {
                let remote_runtime: USBRemoteRuntime<(), ()> = USBRemoteRuntime::connect(device)?;
                println!("Device: {:?}", remote_runtime.device_serial());
                remote_runtime.disconnect()?;
            }
        }
        Err(e) => {
            log::error!("Failed to enumerate devices: {:?}", e);
        }
    }
    Ok(())
}

