use crate::{
    Identifier,
    interface::Host,
    message::{
        Command, CommandMessage, Event, HostCommand, HostEvent, HostInfo, Version
    }
};

mod serialization;
pub mod usb;

use std::sync::Mutex;
use usb::{
    USBDevice,
    USBRemoteHost
};

pub struct RemoteHost { 
    usb_remote_host: Option<Mutex<USBRemoteHost>>
}

impl RemoteHost {
    pub fn attached() -> Result<Vec<Self>, crate::Error> {
        let devices = USBDevice::attached()?;
        devices.into_iter()
            .map(|device| Self::connect(device))
            .collect()
    }

    pub fn connect(device: USBDevice) -> Result<Self, crate::Error> {
        let usb_remote_host = USBRemoteHost::connect(device)?;
        Ok(Self {
            usb_remote_host: Some(Mutex::new(usb_remote_host))
        })
    }

    pub fn disconnect(&mut self) -> Result<(), crate::Error> {
        if let Some(usb_remote_host) = self.usb_remote_host.take() {
            let usb_remote_host = usb_remote_host.into_inner()
                .map_err(|_e| { crate::Error::Debug("Disconnect called while usb_remote_host lock held".to_string())})?;
            usb_remote_host.disconnect()?;
        }
        Ok(())
    }

    fn ensure_connected(&self) -> Result<(), crate::Error> {
        if self.usb_remote_host.is_none() {
            return Err(crate::Error::USB(
                rusb::Error::NoDevice
            ));
        }
        Ok(())
    }
    
    pub async fn host_info(&self) -> Result<HostInfo, crate::Error> {
        self.ensure_connected()?;
        let mut usb_remote_host = self.usb_remote_host
            .as_ref()
            .expect("USBRemoteHost unexpectedly disconnected")
            .lock()
            .map_err(|_e| crate::Error::Debug("failed to acquire USBRemoteHost lock".to_string()))?;

        let command_message = CommandMessage::root(
            Command::Host(
                HostCommand::Info
            ),
            None
        );
        let event_message = usb_remote_host.execute_command(command_message).await?;
        let Event::Host(HostEvent::Info(host_info)) = event_message.event else {
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

impl Drop for RemoteHost {
    fn drop(&mut self) {
        self.disconnect().expect("Failed to disconnect from RemoteHost during Drop");
    }
}

impl Host for RemoteHost {
    fn version(&self) -> Version {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { RemoteHost::version(self).await })
            .unwrap()
    }

    fn identifier(&self) -> Identifier {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { RemoteHost::identifier(self).await })
            .unwrap()
    }
}