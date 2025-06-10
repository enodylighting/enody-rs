use enody::{
    message::{Command, CommandMessage, Flux, Message},
    remote::{USBDevice, USBDeviceEvent, USBRemoteRuntime}, Chromaticity, Configuration,
};
use log;
use tokio::{
    runtime::Runtime,
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use uuid::Uuid;

use eframe::egui;

fn main() -> eframe::Result {
    env_logger::init();
    log::info!("enody rust example application");

    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Enody",
        options,
        Box::new(|_cc| Ok(Box::new(EnodyApplication::new()))),
    )
}

enum LastColorMode {
    Chromatic,
    Blackbody,
}

struct DeviceState {
    pub chromaticity: Chromaticity,
    pub flux: f32,
    pub cct: f32,
    pub last_color_mode: LastColorMode,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            chromaticity: Chromaticity { x: 0.33, y: 0.33 },
            flux: 0.1,
            cct: 6500.0,
            last_color_mode: LastColorMode::Blackbody,
        }
    }
}

struct EnodyApplication {
    _tokio_runtime: Runtime,
    devices: Vec<(USBDevice, DeviceState)>,
    connection_event_rx: mpsc::Receiver<USBDeviceEvent>,
    runtime_message_rx: mpsc::Receiver<(USBDevice, Message)>,
    command_tx: broadcast::Sender<(USBDevice, CommandMessage<()>)>
}

impl EnodyApplication {
    pub fn new() -> Self {
        let tokio_runtime = Runtime::new().unwrap();
        let (connection_event_tx, connection_event_rx) = mpsc::channel(4);
        let (task_runtime_message_tx, runtime_message_rx) = mpsc::channel(4);
        let (command_tx, _) = broadcast::channel(4);
        let command_tx_clone = command_tx.clone();

        tokio_runtime.spawn(async move {
            let mut usb_monitor = enody::remote::USBDeviceMonitor::new();
            let (shutdown_tx, _) = broadcast::channel(1);

            let (device_event_tx, mut device_event_rx) = tokio::sync::mpsc::channel(4);
            if let Err(e) = usb_monitor.start(Some(device_event_tx)) {
                log::error!("Failed to start USB monitor: {:?}", e);
                return;
            }

            let mut runtime_tasks: Vec<(USBDevice, JoinHandle<()>)> = Vec::new();

            let mut handle_device_event = async |device_event: USBDeviceEvent| {
                log::info!("USBDeviceEvent: {:?}", device_event);
                if connection_event_tx.send(device_event.clone()).await.is_err() {
                    log::error!("Failed to send device arrived to UI: channel closed.");
                    return;
                }

                match device_event {
                    USBDeviceEvent::Arrived(device) => {
                        let mut runtime = match USBRemoteRuntime::<(), ()>::open(device.clone(), shutdown_tx.subscribe()) {
                            Ok(r) => r,
                            Err(e) => {
                                log::error!("failed to open device: {:?}, error: {:?}", device, e);
                                return;
                            }
                        };

                        let task_message_tx = task_runtime_message_tx.clone();
                        let task_device = device.clone();
                        let mut command_rx = command_tx_clone.subscribe();
                        let task = tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    Ok(message) = runtime.next_message() => {
                                        if let Err(e) = task_message_tx.send((task_device.clone(), message)).await {
                                            log::error!("failed to send runtime message: {:?}", e);
                                        }
                                    }
                                    Ok((cmd_dev, cmd_message)) = command_rx.recv() => {
                                        if cmd_dev == task_device {
                                            if let Err(e) = runtime.execute_command(cmd_message).await {
                                                log::error!("Error executing command: {:?}", e);
                                            }
                                        }
                                    }
                                    else => {
                                        break;
                                    }
                                }
                            }
                        });

                        // Store the task with the device
                        log::info!("Started task for device: {:?}", device);
                        runtime_tasks.push((device, task));
                    }
                    USBDeviceEvent::Left(device) => {
                        // If we have a task for this device, abort it and remove from our map
                        if let Some(index) = runtime_tasks.iter().position(|(d, _)| d == &device) {
                            let (_, task) = runtime_tasks.swap_remove(index);
                            log::info!("Aborting task for removed device: {:?}", device);
                            task.abort();
                        }
                    }
                }
            };

            while let Some(device_event) = device_event_rx.recv().await {
                handle_device_event(device_event).await;
            }
        });

        Self {
            _tokio_runtime: tokio_runtime,
            devices: Vec::new(),
            connection_event_rx,
            runtime_message_rx,
            command_tx
        }
    }
}

impl eframe::App for EnodyApplication {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {

        let uuid_gen = |index: u16| {
            let timestamp = uuid::Timestamp::from_gregorian(0, index);
            uuid::Uuid::new_v6(timestamp, &[0; 6])
        };

        // handle connection events
        while let Ok(connection_event) = self.connection_event_rx.try_recv() {
            match connection_event {
                USBDeviceEvent::Arrived(device) => {
                    self.devices.push((device, DeviceState::default()));
                },
                USBDeviceEvent::Left(device) => {
                    if let Some(pos) = self.devices.iter().position(|d| d.0 == device) {
                        self.devices.swap_remove(pos);
                    }
                }
            }
        }

        // handle runtime messages
        while let Ok((_device, message)) = self.runtime_message_rx.try_recv() {
            log::info!("{:?}", message);
        }

        let device_interface = |device: &USBDevice, device_state: &mut DeviceState, ui: &mut egui::Ui| {
            ui.label(device.identifier().name);
            ui.end_row();
            
            // First row: chromaticity controls
            if ui.add(egui::Slider::new(&mut device_state.chromaticity.x, 0.0..=1.0).prefix("x:")).changed()
                || ui.add(egui::Slider::new(&mut device_state.chromaticity.y, 0.0..=1.0).prefix("y:")).changed() {
                    device_state.last_color_mode = LastColorMode::Chromatic;
                    for source in 0..2 {
                        let cmd = CommandMessage {
                            identifier: Uuid::new_v4(),
                            context: None,
                            resource: Some(uuid_gen(source as u16)),
                            command: Command::Source(
                                enody::message::SourceCommand::Display(
                                    Configuration::Chromatic(
                                        device_state.chromaticity.clone()
                                    ),
                                    Flux::Relative(device_state.flux)

                                )
                            )
                        };
                        let _ = self.command_tx.send((device.clone(), cmd));
                    }
            };
            ui.end_row();
            
            // Second row: flux and CCT controls
            let flux_changed = ui.add(egui::Slider::new(&mut device_state.flux, 0.0..=1.0).fixed_decimals(3).prefix("flux:")).changed();
            let cct_changed = ui.add(egui::Slider::new(&mut device_state.cct, 1_200.0..=6_500.0).fixed_decimals(0).prefix("cct (K):")).changed();
            
            if flux_changed {
                for source in 0..2 {
                    let cmd = CommandMessage {
                        identifier: Uuid::new_v4(),
                        context: None,
                        resource: Some(uuid_gen(source as u16)),
                        command: Command::Source(
                            enody::message::SourceCommand::Display(
                                match device_state.last_color_mode {
                                    LastColorMode::Chromatic => Configuration::Chromatic(
                                        device_state.chromaticity.clone()
                                    ),
                                    LastColorMode::Blackbody => Configuration::Blackbody(device_state.cct),
                                },
                                Flux::Relative(device_state.flux)
                            )
                        )
                    };
                    let _ = self.command_tx.send((device.clone(), cmd));
                }
            }
            
            if cct_changed {
                device_state.last_color_mode = LastColorMode::Blackbody;
                for source in 0..2 {
                    let cmd = CommandMessage {
                        identifier: Uuid::new_v4(),
                        context: None,
                        resource: Some(uuid_gen(source as u16)),
                        command: Command::Source(
                            enody::message::SourceCommand::Display(
                                Configuration::Blackbody(device_state.cct),
                                Flux::Relative(device_state.flux)
                            )
                        )
                    };
                    let _ = self.command_tx.send((device.clone(), cmd));
                }
            }
            ui.end_row();
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Enody Devices");

            if self.devices.is_empty() {
                ui.label("No devices connected.");
            } else {
                egui::Grid::new("devices_grid")
                    .num_columns(4)
                    .spacing([40.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.style_mut().spacing.slider_width = 300.0;

                        ui.label("Device ID");
                        ui.end_row();

                        for (device, device_state) in &mut self.devices {
                            device_interface(&device, device_state, ui);
                        }
                    });
            }
        });

        ctx.request_repaint();
    }
}
