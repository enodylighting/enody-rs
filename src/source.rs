//! Source-level traits and remote source handles.
//!
//! A source is an independently controllable region within a fixture. Sources
//! can display their own computed configuration or expose individual emitters.

use alloc::boxed::Box;

use crate::{
    emitter::Emitter,
    message::{Configuration, Flux, SourceState, Transition},
    Error, Identifier,
};

/// Represents a light source containing one or more emitters.
#[allow(clippy::result_large_err)]
pub trait Source: Send + Sync {
    /// Returns the stable source identifier.
    fn identifier(&self) -> Identifier;

    /// Displays a configuration at a target flux on the source.
    fn display(
        &mut self,
        config: Configuration,
        target_flux: Flux,
    ) -> Result<(Configuration, Flux), Error>;

    /// Transitions this source to a target state over time.
    ///
    /// Local implementations may override this to run the interpolation on the
    /// device side. The default returns [`Error::Unsupported`].
    fn transition(&mut self, transition: Transition<SourceState>) -> Result<SourceState, Error> {
        let _ = transition;
        Err(Error::Unsupported)
    }

    /// Returns the emitters contained by this source.
    fn emitters(&self) -> &[Box<dyn Emitter>];
}

#[cfg(feature = "remote")]
/// Remote source handles.
pub mod remote {
    use crate::{
        emitter::remote::RemoteEmitter,
        message::{
            Command, CommandMessage, Configuration, EmitterInfo, Event, Flux, SourceCommand,
            SourceEvent, SourceInfo, SourceState, Transition,
        },
        runtime::remote::RemoteRuntime,
        Identifier,
    };

    /// A source accessed via remote runtime communication.
    ///
    /// RemoteSource wraps a cloned RemoteRuntime and provides access to
    /// source operations through the command/event protocol.
    /// Source accessed through a [`RemoteRuntime`].
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

        /// Fetch the source's current target state.
        pub async fn state(&self) -> Result<SourceState, crate::Error> {
            let command = Command::Source(SourceCommand::State);
            let command_message = CommandMessage::root(command, Some(self.identifier()));

            let event_message = self.remote.execute_command(command_message).await?;
            if event_message.resource != Some(self.identifier()) {
                return Err(crate::Error::UnexpectedResponse);
            }

            match event_message.event {
                Event::Source(SourceEvent::State(state)) => Ok(state),
                _ => Err(crate::Error::UnexpectedResponse),
            }
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

        /// Run a source transition and wait for its terminal event.
        ///
        /// The remote runtime may also emit a context-matched
        /// [`SourceEvent::TransitionStart`] before the transition ends. That
        /// intermediate event is consumed while this method waits for a
        /// [`SourceEvent::TransitionEnd`] that carries the same transition
        /// payload as this request.
        pub async fn transition(
            &self,
            transition: Transition<SourceState>,
        ) -> Result<SourceState, crate::Error> {
            let expected_transition = transition.clone();
            let timeout = transition
                .method
                .duration()
                .saturating_add(std::time::Duration::from_secs(2));
            let command = Command::Source(SourceCommand::Transition(transition));
            let command_message = CommandMessage::root(command, Some(self.identifier()));
            let context = command_message.identifier;

            let event_message = self
                .remote
                .execute_command_with_timeout_until(
                    command_message,
                    timeout,
                    move |event_message| {
                        event_message.context.as_ref() == Some(&context)
                            && matches!(
                                &event_message.event,
                                Event::Source(SourceEvent::TransitionEnd(transition, _))
                                    if transition == &expected_transition
                            )
                    },
                )
                .await?;
            if event_message.resource != Some(self.identifier()) {
                return Err(crate::Error::UnexpectedResponse);
            }

            match event_message.event {
                Event::Source(SourceEvent::TransitionEnd(_, state)) => Ok(state),
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
