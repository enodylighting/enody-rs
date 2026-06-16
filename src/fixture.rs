//! Fixture-level traits and remote fixture handles.
//!
//! A fixture is an addressable light output unit. It can display a computed
//! configuration across all of its sources or expose those sources for more
//! granular control.

use alloc::boxed::Box;

use crate::{
    message::{Configuration, FixtureState, Flux, Transition},
    source::Source,
    Error, Identifier,
};

/// Represents a fixture containing one or more light sources.
#[allow(clippy::result_large_err)]
pub trait Fixture: Send + Sync {
    /// Returns the stable fixture identifier.
    fn identifier(&self) -> Identifier;

    /// Displays a configuration at a target flux on the entire fixture.
    fn display(
        &mut self,
        config: Configuration,
        target_flux: Flux,
    ) -> Result<(Configuration, Flux), Error>;

    /// Transitions the fixture to a target state over time.
    ///
    /// Local implementations may override this to run the interpolation on the
    /// device side. The default returns [`Error::Unsupported`].
    fn transition(&mut self, transition: Transition<FixtureState>) -> Result<FixtureState, Error> {
        let _ = transition;
        Err(Error::Unsupported)
    }

    /// Returns the sources contained by this fixture.
    fn sources(&self) -> &[Box<dyn Source>];
}

#[cfg(feature = "remote")]
/// Remote fixture handles.
pub mod remote {
    use crate::{
        message::{
            Command, CommandMessage, Configuration, Event, FixtureCommand, FixtureEvent,
            FixtureInfo, FixtureState, Flux, SourceInfo, Transition,
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

        /// Fetch the fixture's current target state.
        pub async fn state(&self) -> Result<FixtureState, crate::Error> {
            let command = Command::Fixture(FixtureCommand::State);
            let command_message = CommandMessage::root(command, Some(self.identifier()));

            let event_message = self.remote.execute_command(command_message).await?;
            if event_message.resource != Some(self.identifier()) {
                return Err(crate::Error::UnexpectedResponse);
            }

            match event_message.event {
                Event::Fixture(FixtureEvent::State(state)) => Ok(state),
                _ => Err(crate::Error::UnexpectedResponse),
            }
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

        /// Run a fixture transition and wait for its terminal event.
        ///
        /// The remote runtime may also emit a context-matched
        /// [`FixtureEvent::TransitionStart`] before the transition ends. That
        /// intermediate event is consumed while this method waits for a
        /// [`FixtureEvent::TransitionEnd`] that carries the same transition
        /// payload as this request.
        pub async fn transition(
            &self,
            transition: Transition<FixtureState>,
        ) -> Result<FixtureState, crate::Error> {
            let expected_transition = transition.clone();
            let timeout = transition
                .method
                .duration()
                .saturating_add(std::time::Duration::from_secs(2));
            let command = Command::Fixture(FixtureCommand::Transition(transition));
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
                                Event::Fixture(FixtureEvent::TransitionEnd(transition, _))
                                    if transition == &expected_transition
                            )
                    },
                )
                .await?;
            if event_message.resource != Some(self.identifier()) {
                return Err(crate::Error::UnexpectedResponse);
            }

            match event_message.event {
                Event::Fixture(FixtureEvent::TransitionEnd(_, state)) => Ok(state),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }
    }
}
