use log;
use tokio;

#[tokio::main]
async fn main() -> Result<(), enody::Error> {
    env_logger::init();
    log::info!("enody rust example application");

    let mut usb_monitor = enody::remote::USBDeviceMonitor::new();
    usb_monitor.start(None)?;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let devices = usb_monitor.connected_devices();
    if devices.len() > 0 {
        let device = devices[0].clone();
        let mut runtime = enody::remote::USBRemoteRuntime::<(), ()>::open(device, usb_monitor.shutdown_rx()).unwrap();

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
            runtime.execute_command(led_command((index + 15) % 16, 0.0)).await.unwrap();
            runtime.execute_command(led_command(index, 0.8)).await.unwrap();
            runtime.execute_command(display_command.clone()).await.unwrap();
        };

        let mut benchmark = async |count: u32| {
            let start = std::time::Instant::now();
            for i in 0..count {
                toggle_led(i as u16 % 16).await;
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
            let duration = start.elapsed();
            let fps = count as f64 / duration.as_secs_f64();
            log::info!("Sent {} commands in {:?} ({:?} per command, {:.2} fps)", count, duration, duration / count, fps);
        };

        benchmark(32).await;

        let chromatic_command = enody::message::CommandMessage {
            identifier: uuid::Uuid::new_v4(),
            context: None,
            resource: Some(uuid_gen(0)),
            command: enody::message::Command::Source(
                enody::message::SourceCommand::Display(
                    enody::Configuration::Chromatic(
                        enody::Chromaticity { x: 0.3127, y: 0.3290 }
                    ),
                    enody::message::Flux::Relative(0.8)
                )
            )
        };
        runtime.execute_command(chromatic_command).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let chromatic_command = enody::message::CommandMessage {
            identifier: uuid::Uuid::new_v4(),
            context: None,
            resource: Some(uuid_gen(0)),
            command: enody::message::Command::Source(
                enody::message::SourceCommand::Display(
                    enody::Configuration::Chromatic(
                        enody::Chromaticity { x: 0.4474, y: 0.4074 }
                    ),
                    enody::message::Flux::Relative(0.8)
                )
            )
        };
        runtime.execute_command(chromatic_command).await.unwrap();
    }

    usb_monitor.stop().await.expect("failed to stop usb monitor");

    Ok(())
}
