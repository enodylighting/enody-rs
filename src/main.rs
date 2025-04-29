use log;
use tokio;

#[tokio::main]
async fn main() -> Result<(), enody::Error> {
    env_logger::init();
    log::info!("enody rust example application");

    let mut usb_monitor = enody::remote::USBDeviceMonitor::new();
    usb_monitor.start()?;

    for _ in 0..10 {
        log::info!("connected devices: {:?}", usb_monitor.connected_devices());
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    usb_monitor.stop().await;
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    Ok(())
}
