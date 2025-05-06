use enody::interface::Recipient;
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

        let uuid_gen = |index: u16| {
            let timestamp = uuid::Timestamp::from_gregorian(0, index);
            uuid::Uuid::new_v6(timestamp, &[0; 6])
        };

        let led_command = |index: u16, flux: f32| {            
            enody::message::CommandMessage {
                identifier: uuid::Uuid::new_v4(),
                context: None,
                resource: Some(uuid_gen(index)),
                command: enody::message::Command::Emitter(
                    enody::message::EmitterCommand::FluxSet(
                        enody::message::Flux::Relative(flux)
                    )
                )
            }
        };

        let display_command = enody::message::CommandMessage {
            identifier: uuid::Uuid::new_v4(),
            context: None,
            resource: Some(uuid_gen(0)),
            command: enody::message::Command::Source(
                enody::message::SourceCommand::Display(
                    enody::Configuration::Manual,
                    enody::message::Flux::Relative(0.0)
                )
            )
        };

        let mut toggle_led = async |index| {
            runtime.execute_command(display_command.clone()).await.unwrap();
            runtime.execute_command(led_command(index, 0.5)).await.unwrap();
            runtime.execute_command(led_command(index, 0.0)).await.unwrap();
            runtime.execute_command(display_command.clone()).await.unwrap();
        };

        let mut benchmark = async |count: u32| {
            let start = std::time::Instant::now();
            for i in 0..count {
                toggle_led(i as u16 % 16).await;
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
