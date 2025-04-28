use log;
use tokio;

#[tokio::main]
async fn main() -> Result<(), enody::Error> {
    env_logger::init();
    log::info!("enody rust example application");

    let mut usb_monitor = enody::remote::USBDeviceMonitor::new();
    usb_monitor.listen()?;

    // Wait forever by creating a future that never completes
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    usb_monitor.close().await;
    
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    Ok(())
}
