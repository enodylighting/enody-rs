use alloc::boxed::Box;

use crate::{
    Identifier,
    fixture::Fixture,
    message::Version
};

/// Represents a host device that can provide access to fixtures.
///
/// A host is typically a physical device (like EP01) that contains
/// one or more fixtures.
pub trait Host: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn version(&self) -> Version;
    fn fixtures(&self) -> &[Box<dyn Fixture>];
}

#[cfg(feature = "remote")]
pub mod remote {
    use crate::{
        Identifier,
        fixture::remote::RemoteFixture,
        message::{
            Command, CommandMessage, Event, FixtureInfo, HostCommand, HostEvent, HostInfo,
            Version
        },
        runtime::remote::RemoteRuntime
    };

    /// A host accessed via remote runtime communication.
    ///
    /// RemoteHost wraps a cloned RemoteRuntime and provides high-level access
    /// to host information and fixtures through the command/event protocol.
    /// Since RemoteRuntime implements Clone with a shared internal connection,
    /// multiple RemoteHosts can share the same underlying connection.
    pub struct RemoteHost {
        id: Identifier,
        info: HostInfo,
        remote: RemoteRuntime,
    }

    impl RemoteHost {
        /// Create a new RemoteHost with the given runtime and host info.
        pub fn new(id: Identifier, info: HostInfo, remote: RemoteRuntime) -> Self {
            Self { id, info, remote }
        }

        /// Get the host identifier.
        pub fn identifier(&self) -> Identifier {
            self.id
        }

        /// Get the host version.
        pub fn version(&self) -> Version {
            self.info.version.clone()
        }

        /// Create a RemoteHost by querying the device for its host information.
        pub async fn from_runtime(remote: RemoteRuntime) -> Result<Self, crate::Error> {
            let info = Self::fetch_host_info(&remote).await?;
            let id = info.identifier;
            Ok(Self::new(id, info, remote))
        }

        /// Fetch host information from the remote device.
        async fn fetch_host_info(remote: &RemoteRuntime) -> Result<HostInfo, crate::Error> {
            let command = Command::Host(HostCommand::Info);
            let command_message = CommandMessage::root(command, None);

            let event_message = remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Host(HostEvent::Info(host_info)) => Ok(host_info),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Get the host info structure.
        pub fn host_info(&self) -> &HostInfo {
            &self.info
        }

        /// Get a clone of the RemoteRuntime for creating child resources.
        pub fn runtime(&self) -> RemoteRuntime {
            self.remote.clone()
        }

        /// Fetch the number of fixtures from the remote device.
        async fn fixture_count(&self) -> Result<u32, crate::Error> {
            let command = Command::Host(HostCommand::FixtureCount);
            let command_message = CommandMessage::root(command, None);

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Host(HostEvent::FixtureCount(count)) => Ok(count),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Fetch information about a specific fixture by index.
        async fn fixture_info(&self, index: u32) -> Result<FixtureInfo, crate::Error> {
            let command = Command::Host(HostCommand::FixtureInfo(index));
            let command_message = CommandMessage::root(command, None);

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Host(HostEvent::FixtureInfo(info)) => Ok(info),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Discover and create RemoteFixture objects for all fixtures on this host.
        pub async fn fixtures(&self) -> Result<Vec<RemoteFixture>, crate::Error> {
            let count = self.fixture_count().await?;
            let mut fixtures = Vec::with_capacity(count as usize);

            for i in 0..count {
                let info = self.fixture_info(i).await?;
                let fixture = RemoteFixture::new(
                    info.identifier,
                    info,
                    self.remote.clone(),
                );
                fixtures.push(fixture);
            }

            Ok(fixtures)
        }
    }

}
