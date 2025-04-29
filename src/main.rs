use enody::message::Responder;
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
        let device = devices[0].clone();
        let mut runtime = enody::remote::USBRemoteRuntime::open(device, usb_monitor.shutdown_rx()).unwrap();
        let command = enody::message::CommandMessage {
            identifier: uuid::Uuid::new_v4(),
            context: None,
            resource: None,
            command: enody::message::Command::Host(
                enody::message::HostCommand::Info
            )
        };

        let mut benchmark = async |count: u32| {
            let start = std::time::Instant::now();
            for i in 0..count {
                runtime.handle_command(command.clone());
    
                let response = runtime.next_message().await.unwrap();
                // log::info!("{:?}", response);
            }
            let duration = start.elapsed();
            log::info!("Sent {} commands in {:?} ({:?} per command)", count, duration, duration / count);
        };

        benchmark(1).await;
        benchmark(10).await;
        benchmark(100).await;
        benchmark(1_000).await;
    }

    usb_monitor.stop().await.expect("failed to stop usb monitor");

    Ok(())
}
