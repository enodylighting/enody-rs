use alloc::boxed::Box;

use crate::{
    emitter::Emitter,
    message::{Configuration, Flux},
    Error, Identifier,
};

/// Represents a light source containing one or more emitters.
#[allow(clippy::result_large_err)]
pub trait Source: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn display(
        &mut self,
        config: Configuration,
        target_flux: Flux,
    ) -> Result<(Configuration, Flux), Error>;
    fn emitters(&self) -> &[Box<dyn Emitter>];
}

#[cfg(feature = "remote")]
pub mod remote {
    use crate::{
        emitter::remote::RemoteEmitter,
        message::{
            Command, CommandMessage, Configuration, EmitterInfo, Event, Flux, SourceCommand,
            SourceEvent, SourceInfo,
        },
        runtime::remote::RemoteRuntime,
        Identifier,
    };

    /// A source accessed via remote runtime communication.
    ///
    /// RemoteSource wraps a cloned RemoteRuntime and provides access to
    /// source operations through the command/event protocol.
    pub struct RemoteSource {
        info: SourceInfo,
        remote: RemoteRuntime,
    }

    impl RemoteSource {
        /// Create a new RemoteSource with the given runtime and source info.
        pub fn new(info: SourceInfo, remote: RemoteRuntime) -> Self {
            Self { info, remote }
        }

        /// Get the source identifier.
        pub fn identifier(&self) -> Identifier {
            self.info.identifier
        }

        /// Fetch the number of emitters in this source.
        pub async fn emitter_count(&self) -> Result<u32, crate::Error> {
            let command = Command::Source(SourceCommand::EmitterCount);
            let command_message = CommandMessage::root(command, Some(self.identifier()));

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Source(SourceEvent::EmitterCount(count)) => Ok(count),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Send a display command to the source.
        pub async fn display(
            &self,
            config: Configuration,
            target_flux: Flux,
        ) -> Result<(Configuration, Flux), crate::Error> {
            let command = Command::Source(SourceCommand::Display(config, target_flux));
            let command_message = CommandMessage::root(command, Some(self.identifier()));

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Source(SourceEvent::Display(config, flux)) => Ok((config, flux)),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Fetch information about a specific emitter by index.
        async fn emitter_info(&self, index: u32) -> Result<EmitterInfo, crate::Error> {
            let command = Command::Source(SourceCommand::EmitterInfo(index));
            let command_message = CommandMessage::root(command, Some(self.identifier()));

            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Source(SourceEvent::EmitterInfo(info)) => Ok(info),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Discover and create RemoteEmitter objects for all emitters on this source.
        pub async fn emitters(&self) -> Result<Vec<RemoteEmitter>, crate::Error> {
            let count = self.emitter_count().await?;
            let mut emitters = Vec::with_capacity(count as usize);

            for i in 0..count {
                let info = self.emitter_info(i).await?;
                let emitter = RemoteEmitter::new(info, self.remote.clone());
                emitters.push(emitter);
            }

            Ok(emitters)
        }
    }
}
