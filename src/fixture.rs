use alloc::boxed::Box;

use crate::{
    message::{Configuration, Flux},
    source::Source,
    Error, Identifier,
};

/// Represents a fixture containing one or more light sources.
pub trait Fixture: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn display(
        &mut self,
        config: Configuration,
        target_flux: Flux,
    ) -> Result<(Configuration, Flux), Error>;
    fn sources(&self) -> &[Box<dyn Source>];
}

#[cfg(feature = "remote")]
pub mod remote {
    use crate::{
        message::{
            Command, CommandMessage, Configuration, Event, FixtureCommand, FixtureEvent,
            FixtureInfo, Flux, SourceInfo,
        },
        runtime::remote::RemoteRuntime,
        source::remote::RemoteSource,
        Identifier,
    };

    /// A fixture accessed via remote runtime communication.
    ///
    /// RemoteFixture wraps a cloned RemoteRuntime and provides access to
    /// fixture operations through the command/event protocol.
    #[derive(Clone, Debug)]
    pub struct RemoteFixture {
        info: FixtureInfo,
        remote: RemoteRuntime,
    }

    impl RemoteFixture {
        /// Create a new RemoteFixture with the given runtime and fixture info.
        pub fn new(info: FixtureInfo, remote: RemoteRuntime) -> Self {
            Self { info, remote }
        }

        /// Fetch the number of sources in this fixture.
        pub async fn source_count(&self) -> Result<u32, crate::Error> {
            let command = Command::Fixture(FixtureCommand::SourceCount);
            let command_message = CommandMessage::root(command, Some(self.identifier()));

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Fixture(FixtureEvent::SourceCount(count)) => Ok(count),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Get the fixture identifier.
        pub fn identifier(&self) -> Identifier {
            self.info.identifier
        }

        /// Fetch information about a specific source by index.
        async fn source_info(&self, index: u32) -> Result<SourceInfo, crate::Error> {
            let command = Command::Fixture(FixtureCommand::SourceInfo(index));
            let command_message = CommandMessage::root(command, Some(self.identifier()));

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Fixture(FixtureEvent::SourceInfo(info)) => Ok(info),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Discover and create RemoteSource objects for all sources on this fixture.
        pub async fn sources(&self) -> Result<Vec<RemoteSource>, crate::Error> {
            let count = self.source_count().await?;
            let mut sources = Vec::with_capacity(count as usize);

            for i in 0..count {
                let info = self.source_info(i).await?;
                let source = RemoteSource::new(info, self.remote.clone());
                sources.push(source);
            }

            Ok(sources)
        }

        /// Send a display command to the fixture.
        pub async fn display(
            &self,
            config: Configuration,
            target_flux: Flux,
        ) -> Result<(Configuration, Flux), crate::Error> {
            let command = Command::Fixture(FixtureCommand::Display(config, target_flux));
            let command_message = CommandMessage::root(command, Some(self.identifier()));

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Fixture(FixtureEvent::Display(config, flux)) => Ok((config, flux)),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }
    }
}
