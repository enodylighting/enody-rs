use std::time::Duration;

use log;
use tokio;

#[tokio::main]
async fn main() -> Result<(), enody::Error> {
    env_logger::init();
    log::info!("enody rust example application");

    let mut usb_monitor = enody::remote::USBDeviceMonitor::new();
    usb_monitor.start()?;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let devices = usb_monitor.connected_devices();
    if devices.len() > 0 {
        let mut device = devices[0].clone();
        let handle = device.initialize_device().unwrap();

        let message = enody::message::Message::Command(
            enody::message::CommandMessage {
                identifier: uuid::Uuid::new_v4(),
                context: None,
                resource: None,
                command: enody::message::Command::Host(
                    enody::message::HostCommand::Info
                )
            }
        );

        let payload: Vec<u8> = Vec::<u8>::try_from(message).unwrap();
        handle.write_bulk(0x01, &payload, Duration::ZERO).unwrap();
    }

    usb_monitor.stop().await.expect("failed to stop usb monitor");

    Ok(())
}
