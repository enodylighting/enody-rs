use alloc::boxed::Box;

use crate::{
    Error,
    Identifier,
    interface::Source,
    message::{Configuration, Flux},
};

/// Represents a fixture containing one or more light sources.
pub trait Fixture: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn display(&mut self, config: Configuration, target_flux: Flux) -> Result<(Configuration, Flux), Error>;
    fn sources(&self) -> &[Box<dyn Source>];
}

#[cfg(feature = "remote")]
pub mod remote {
    use crate::{
        Identifier,
        message::{
            Command, CommandMessage, Configuration, Event, Flux,
            FixtureCommand, FixtureEvent, FixtureInfo,
        },
        runtime::remote::RemoteRuntime,
    };

    /// A fixture accessed via remote runtime communication.
    ///
    /// RemoteFixture wraps a cloned RemoteRuntime and provides access to
    /// fixture operations through the command/event protocol.
    pub struct RemoteFixture {
        id: Identifier,
        info: FixtureInfo,
        remote: RemoteRuntime,
    }

    impl RemoteFixture {
        /// Create a new RemoteFixture with the given runtime and fixture info.
        pub fn new(id: Identifier, info: FixtureInfo, remote: RemoteRuntime) -> Self {
            Self { id, info, remote }
        }

        /// Fetch the number of sources in this fixture.
        pub async fn source_count(&self) -> Result<u32, crate::Error> {
            let command = Command::Fixture(FixtureCommand::SourceCount);
            let command_message = CommandMessage::root(command, Some(self.id));

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Fixture(FixtureEvent::SourceCount(count)) => Ok(count),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Get the fixture identifier.
        pub fn identifier(&self) -> Identifier {
            self.id
        }

        /// Get the fixture info structure.
        pub fn fixture_info(&self) -> &FixtureInfo {
            &self.info
        }

        /// Get a clone of the RemoteRuntime for creating child resources.
        pub fn runtime(&self) -> RemoteRuntime {
            self.remote.clone()
        }

        /// Send a display command to the fixture.
        pub async fn display(&self, config: Configuration, target_flux: Flux) -> Result<(Configuration, Flux), crate::Error> {
            let command = Command::Fixture(FixtureCommand::Display(config, target_flux));
            let command_message = CommandMessage::root(command, Some(self.id));

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Fixture(FixtureEvent::Display(config, flux)) => Ok((config, flux)),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }
    }
}
