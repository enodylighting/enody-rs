use crate::{
    Identifier,
    message::{
        Command, CommandMessage, Event, HostCommand, HostEvent, HostInfo, Version
    }
};

mod serialization;
pub mod usb;

use std::sync::Mutex;
use usb::{
    USBDevice,
    USBRemoteRuntime
};

pub struct RemoteRuntime {
    usb_remote_runtime: Option<Mutex<USBRemoteRuntime>>,
}

impl RemoteRuntime {
    pub fn attached() -> Result<Vec<Self>, crate::Error> {
        let devices = USBDevice::attached()?;
        devices.into_iter()
            .map(|device| Self::connect(device))
            .collect()
    }

    pub fn connect(device: USBDevice) -> Result<Self, crate::Error> {
        let usb_remote_runtime = USBRemoteRuntime::connect(device)?;
        Ok(Self {
            usb_remote_runtime: Some(Mutex::new(usb_remote_runtime))
        })
    }

    pub fn disconnect(&mut self) -> Result<(), crate::Error> {
        if let Some(usb_remote_runtime) = self.usb_remote_runtime.take() {
            let mut usb_remote_runtime = usb_remote_runtime.into_inner()
                .map_err(|_e| { crate::Error::Debug("Disconnect called while usb_remote_runtime lock held".to_string())})?;
            usb_remote_runtime.disconnect()?;
        }
        Ok(())
    }

    fn ensure_connected(&self) -> Result<(), crate::Error> {
        if self.usb_remote_runtime.is_none() {
            return Err(crate::Error::USB(
                rusb::Error::NoDevice
            ));
        }
        Ok(())
    }

    async fn execute_command(&self, command: Command, resource: Option<Identifier>) -> Result<(Event, Option<Identifier>), crate::Error> {
        self.ensure_connected()?;
        let mut usb_remote_runtime = self.usb_remote_runtime
            .as_ref()
            .expect("USBRemoteRuntime unexpectedly disconnected")
            .lock()
            .map_err(|_e| crate::Error::Debug("failed to acquire USBRemoteHost lock".to_string()))?;

        let command_message = CommandMessage::root(command, resource);
        let event_message = usb_remote_runtime.execute_command(command_message).await?;
        Ok((event_message.event, event_message.resource))
    }

    pub fn host(&self) -> RemoteHost<'_> {
        RemoteHost::new(self)
    }

    pub fn environments(&self) -> Vec<RemoteEnvironment<'_>> {
        // collect environments via sending command EnvironmentList repeatedly
        Vec::new()
    }
}

impl Drop for RemoteRuntime {
    fn drop(&mut self) {
        self.disconnect().expect("Failed to disconnect from RemoteRuntime during Drop");
    }
}

pub struct RemoteHost<'a> {
    runtime: &'a RemoteRuntime
}

impl<'a> RemoteHost<'a> {
    fn new(runtime: &'a RemoteRuntime) -> Self {
        Self {
            runtime
        }
    }

    pub async fn host_info(&self) -> Result<HostInfo, crate::Error> {
        let command = Command::Host(HostCommand::Info);
        let (event, _) = self.runtime.execute_command(command, None).await?;
        let Event::Host(HostEvent::Info(host_info)) = event else {
            return Err(crate::Error::UnexpectedResponse);
        };
        Ok(host_info)
    }

    pub async fn version(&self) -> Result<Version, crate::Error> {
        let host_info = self.host_info().await?;
        Ok(host_info.version)
    }

    pub async fn identifier(&self) -> Result<Identifier, crate::Error> {
        let host_info = self.host_info().await?;
        Ok(host_info.identifier)
    }
}

pub struct RemoteEnvironment<'a> {
    runtime: &'a RemoteRuntime
}

impl<'a> RemoteEnvironment<'a> {
    fn new(runtime: &'a RemoteRuntime) -> Self {
        Self {
            runtime
        }
    }
}