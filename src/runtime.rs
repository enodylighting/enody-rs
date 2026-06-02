use alloc::boxed::Box;

use crate::{
    host::Host,
    message::{CommandMessage, EventMessage},
};

#[allow(clippy::result_large_err)]
pub trait Runtime {
    fn execute_command(&mut self, message: CommandMessage) -> Result<EventMessage, crate::Error>;
    fn handle_command(&mut self, message: CommandMessage) -> Result<(), crate::Error>;
    fn handle_event(&mut self, message: EventMessage) -> Result<(), crate::Error>;
    fn host(&self) -> Box<dyn Host>;
}

#[cfg(feature = "remote")]
pub mod remote {
    use crate::{
        host::remote::RemoteHost,
        message::{
            Command, CommandMessage, Event, EventMessage, LogLevel, Message, RuntimeCommand,
            RuntimeEvent, SettingKey, SettingValue, StoredSetting, Token,
        },
        Identifier,
    };
    use async_trait::async_trait;

    use std::{
        fmt::Debug,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };
    use tokio::sync::{mpsc, Mutex as AsyncMutex};
    use tokio::task::JoinHandle;

    /// Trait for providing message transport to a remote runtime.
    ///
    /// This trait abstracts the connection layer, handling only the transport
    /// concerns: connecting, disconnecting, and sending/receiving messages.
    /// The connection is a naive message shuttler with no command/response logic.
    /// Implementations must handle their own internal synchronization.
    #[async_trait]
    pub trait RemoteRuntimeConnection: Debug + Send + Sync {
        fn identifier(&self) -> Identifier;

        /// Check if the connection is currently active.
        fn is_connected(&self) -> bool;

        /// Establish a connection to the remote runtime.
        async fn connect(&self) -> Result<(), crate::Error>;

        /// Close the connection to the remote runtime.
        async fn disconnect(&self) -> Result<(), crate::Error>;

        /// Send a message to the remote runtime.
        async fn send_message(&self, message: Message) -> Result<(), crate::Error>;

        /// Receive the next message from the remote runtime.
        /// This should block until a message is available or the connection is closed.
        async fn recv_message(&self) -> Result<Message, crate::Error>;
    }

    /// Registration for a pending command awaiting its response.
    #[derive(Debug)]
    struct CommandResponseRegistration {
        context: Identifier,
        response_tx: mpsc::Sender<EventMessage>,
    }

    /// Shared internal state for RemoteRuntime.
    /// This is wrapped in Arc so that all clones share the same state.
    #[derive(Debug)]
    struct RemoteRuntimeInner {
        connection: Arc<dyn RemoteRuntimeConnection>,
        pending_commands: AsyncMutex<Vec<CommandResponseRegistration>>,
        /// Channel for messages that aren't command responses (e.g., unsolicited events)
        message_tx: tokio::sync::mpsc::Sender<Message>,
        message_rx: AsyncMutex<tokio::sync::mpsc::Receiver<Message>>,
        /// Whether to log incoming Log events using the log crate
        logging_enabled: AtomicBool,
        dispatch_task: AsyncMutex<Option<JoinHandle<()>>>,
    }

    /// A runtime that communicates with remote devices via a connection.
    ///
    /// `RemoteRuntime` wraps a shared connection and provides high-level
    /// operations for interacting with remote runtimes. It implements `Clone`
    /// to allow easy sharing of the connection across multiple remote resources
    /// (hosts, fixtures, sources, emitters).
    ///
    /// The command/response matching logic is handled at this layer, not in the
    /// connection. The connection is just a naive message transport.
    #[derive(Clone, Debug)]
    pub struct RemoteRuntime {
        inner: Arc<RemoteRuntimeInner>,
    }

    impl RemoteRuntime {
        /// Create a new RemoteRuntime wrapping the given connection.
        pub fn new(connection: Box<dyn RemoteRuntimeConnection>) -> Self {
            let (message_tx, message_rx) = tokio::sync::mpsc::channel(64);

            let inner = Arc::new(RemoteRuntimeInner {
                connection: Arc::from(connection),
                pending_commands: AsyncMutex::new(Vec::new()),
                message_tx,
                message_rx: AsyncMutex::new(message_rx),
                logging_enabled: AtomicBool::new(false),
                dispatch_task: AsyncMutex::new(None),
            });

            Self { inner }
        }

        async fn ensure_dispatch_task(inner: &Arc<RemoteRuntimeInner>) {
            let mut task = inner.dispatch_task.lock().await;
            if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
                return;
            }

            if let Some(handle) = task.take() {
                handle.abort();
            }

            let dispatch_inner = inner.clone();
            let handle = tokio::spawn(async move {
                Self::message_dispatch_task(dispatch_inner).await;
            });
            *task = Some(handle);
        }

        async fn stop_dispatch_task(inner: &Arc<RemoteRuntimeInner>) {
            let handle = {
                let mut task = inner.dispatch_task.lock().await;
                task.take()
            };
            if let Some(handle) = handle {
                handle.abort();
                let _ = handle.await;
            }
        }

        async fn remove_pending_registration(inner: &RemoteRuntimeInner, context: &Identifier) {
            let mut pending = inner.pending_commands.lock().await;
            pending.retain(|registration| registration.context != *context);
        }

        fn forward_unmatched_message(inner: &RemoteRuntimeInner, message: Message) -> bool {
            if let Message::Event(ref event) = message {
                if let Event::Runtime(RuntimeEvent::Log(ref log_event)) = event.event {
                    if inner.logging_enabled.load(Ordering::Relaxed) {
                        match log_event.level {
                            LogLevel::Error => log::error!("{}", log_event.output),
                            LogLevel::Warn => log::warn!("{}", log_event.output),
                            LogLevel::Info => log::info!("{}", log_event.output),
                            LogLevel::Debug => log::debug!("{}", log_event.output),
                            LogLevel::Trace => log::trace!("{}", log_event.output),
                        }
                        return true;
                    }
                }
            }

            match inner.message_tx.try_send(message) {
                Ok(_) => true,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    log::trace!("Unmatched message channel full, dropping message");
                    true
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    log::trace!("Message channel closed, dispatch task ending");
                    false
                }
            }
        }

        /// Background task that receives messages from the connection and dispatches them.
        /// Command responses are matched to pending registrations; other messages are
        /// forwarded to the message channel.
        async fn message_dispatch_task(inner: Arc<RemoteRuntimeInner>) {
            loop {
                let message = inner.connection.recv_message().await;
                let message = match message {
                    Ok(msg) => msg,
                    Err(e) => {
                        log::trace!("Message dispatch task ending: {:?}", e);
                        break;
                    }
                };

                if let Message::Event(event) = &message {
                    if let Some(context) = event.context {
                        let response_tx = {
                            let mut pending = inner.pending_commands.lock().await;
                            pending.retain(|registration| !registration.response_tx.is_closed());
                            pending
                                .iter()
                                .find(|registration| registration.context == context)
                                .map(|registration| registration.response_tx.clone())
                        };

                        if let Some(response_tx) = response_tx {
                            if response_tx.send(event.clone()).await.is_ok() {
                                continue;
                            }

                            Self::remove_pending_registration(&inner, &context).await;
                        }
                    }
                }

                if !Self::forward_unmatched_message(&inner, message) {
                    break;
                }
            }
        }

        /// Check if the connection is currently active.
        pub fn is_connected(&self) -> bool {
            self.inner.connection.is_connected()
        }

        /// Establish a connection to the remote runtime.
        pub async fn connect(&self) -> Result<(), crate::Error> {
            self.inner.connection.connect().await?;
            Self::ensure_dispatch_task(&self.inner).await;
            Ok(())
        }

        /// Close the connection to the remote runtime.
        pub async fn disconnect(&self) -> Result<(), crate::Error> {
            Self::stop_dispatch_task(&self.inner).await;
            self.inner.connection.disconnect().await
        }

        /// Enable logging of remote runtime Log events.
        ///
        /// When enabled, incoming `RuntimeEvent::Log` events are automatically
        /// logged using the `log` crate at the appropriate level (error, warn,
        /// info, debug, trace). These events are consumed by the logger and
        /// will not appear in `next_message()`.
        pub fn enable_logging(&self) {
            self.inner.logging_enabled.store(true, Ordering::Relaxed);
        }

        /// Execute a command and wait for its response event.
        ///
        /// Default timeout for a single command attempt.
        const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

        const COMMAND_EVENT_BUFFER: usize = 16;

        /// This method sends a command and blocks until the corresponding
        /// response event is received, matching by context identifier.
        pub async fn execute_command(
            &self,
            command: CommandMessage,
        ) -> Result<EventMessage, crate::Error> {
            self.execute_command_with_timeout(command, Self::COMMAND_TIMEOUT)
                .await
        }

        /// Execute a command with a caller-provided timeout per attempt.
        pub async fn execute_command_with_timeout(
            &self,
            command: CommandMessage,
            timeout: std::time::Duration,
        ) -> Result<EventMessage, crate::Error> {
            self.execute_command_with_timeout_until(command, timeout, |_| true)
                .await
        }

        /// Execute a command and wait until a context-matched event satisfies
        /// `is_terminal`. Intermediate context events are forwarded to
        /// `next_message()` and the timeout covers the whole execution window.
        pub async fn execute_command_with_timeout_until<F>(
            &self,
            command: CommandMessage,
            timeout: std::time::Duration,
            mut is_terminal: F,
        ) -> Result<EventMessage, crate::Error>
        where
            F: FnMut(&EventMessage) -> bool,
        {
            let command_debug = format!("{:?}", command.command);
            let context = command.identifier;
            let (response_tx, mut response_rx) = mpsc::channel(Self::COMMAND_EVENT_BUFFER);

            {
                let mut pending = self.inner.pending_commands.lock().await;
                pending.push(CommandResponseRegistration {
                    context,
                    response_tx,
                });
            }

            let start = std::time::Instant::now();
            let deadline = tokio::time::Instant::now() + timeout;
            log::trace!("Sending command {} (context={})", command_debug, context);
            let message = Message::Command(command);
            if let Err(error) = self.inner.connection.send_message(message).await {
                Self::remove_pending_registration(&self.inner, &context).await;
                return Err(error);
            }

            loop {
                match tokio::time::timeout_at(deadline, response_rx.recv()).await {
                    Ok(Some(response)) => {
                        if response.context.as_ref() == Some(&context) {
                            if let Event::Error(error) = &response.event {
                                Self::remove_pending_registration(&self.inner, &context).await;
                                return Err(error.clone());
                            }
                        }

                        if is_terminal(&response) {
                            Self::remove_pending_registration(&self.inner, &context).await;
                            let elapsed = start.elapsed();
                            log::debug!(
                                "Command {} completed in {} ms (context={})",
                                command_debug,
                                (elapsed.as_micros() as f32 / 1000.0),
                                context,
                            );
                            return Ok(response);
                        }

                        Self::forward_unmatched_message(&self.inner, Message::Event(response));
                    }
                    Ok(None) => {
                        Self::remove_pending_registration(&self.inner, &context).await;
                        return Err(crate::Error::Debug(
                            "command response channel closed".to_string(),
                        ));
                    }
                    Err(_) => {
                        Self::remove_pending_registration(&self.inner, &context).await;
                        log::warn!(
                            "Command {} timed out after {:?} (context={})",
                            command_debug,
                            timeout,
                            context,
                        );
                        return Err(crate::Error::Timeout);
                    }
                }
            }
        }

        /// Send a command message (fire-and-forget).
        pub async fn send_command(&self, command: CommandMessage) -> Result<(), crate::Error> {
            let message = Message::Command(command);
            self.inner.connection.send_message(message).await
        }

        /// Send an event message.
        pub async fn send_event(&self, event: EventMessage) -> Result<(), crate::Error> {
            let message = Message::Event(event);
            self.inner.connection.send_message(message).await
        }

        /// Receive the next unmatched message from the remote runtime.
        /// This returns messages that weren't matched as command responses.
        pub async fn next_message(&self) -> Result<Message, crate::Error> {
            let mut rx = self.inner.message_rx.lock().await;
            match rx.recv().await {
                Some(message) => Ok(message),
                None => Err(crate::Error::Busy),
            }
        }

        /// Create a RemoteHost by querying the device for its host information.
        pub async fn host(&self) -> Result<RemoteHost, crate::Error> {
            RemoteHost::from_runtime(self.clone()).await
        }

        pub async fn generate_token(&self) -> Result<Token, crate::Error> {
            let command = Command::Runtime(RuntimeCommand::TokenGenerate);
            let message = self
                .execute_command_with_timeout_until(
                    CommandMessage::root(command, None),
                    std::time::Duration::from_secs(10),
                    |message| {
                        matches!(
                            message.event,
                            Event::Runtime(RuntimeEvent::TokenGenerated(_))
                        )
                    },
                )
                .await?;
            match message.event {
                Event::Runtime(RuntimeEvent::TokenGenerated(token)) => Ok(token),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        pub async fn setting_get<Value>(&self, key: &str) -> Result<Value, crate::Error>
        where
            Value: serde::de::DeserializeOwned,
        {
            let key = SettingKey::try_from(key).map_err(|_| crate::Error::Argument)?;
            let command = Command::Runtime(RuntimeCommand::SettingGet(key));
            let message = self
                .execute_command(CommandMessage::root(command, None))
                .await?;
            match message.event {
                Event::Runtime(RuntimeEvent::SettingGet(_, StoredSetting::Missing)) => {
                    Err(crate::Error::InsufficientData)
                }
                Event::Runtime(RuntimeEvent::SettingGet(_, StoredSetting::Private)) => {
                    Err(crate::Error::Permission)
                }
                Event::Runtime(RuntimeEvent::SettingGet(_, StoredSetting::Public(bytes))) => {
                    let value = postcard::from_bytes(bytes.as_slice())
                        .map_err(|_| crate::Error::Serialization)?;
                    Ok(value)
                }
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        pub async fn setting_set<Value>(&self, key: &str, value: Value) -> Result<(), crate::Error>
        where
            Value: serde::Serialize,
        {
            let key = SettingKey::try_from(key).map_err(|_| crate::Error::Argument)?;
            let bytes = postcard::to_allocvec(&value).map_err(|_| crate::Error::Serialization)?;
            let setting_value =
                SettingValue::from_slice(&bytes).map_err(|_| crate::Error::Serialization)?;
            let command = Command::Runtime(RuntimeCommand::SettingSet(key, setting_value));
            let message = self
                .execute_command(CommandMessage::root(command, None))
                .await?;
            match message.event {
                Event::Runtime(RuntimeEvent::SettingSet(_)) => Ok(()),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        pub async fn setting_delete(&self, key: &str) -> Result<(), crate::Error> {
            let key = SettingKey::try_from(key).map_err(|_| crate::Error::Argument)?;
            let command = Command::Runtime(RuntimeCommand::SettingDelete(key));
            let message = self
                .execute_command(CommandMessage::root(command, None))
                .await?;
            match message.event {
                Event::Runtime(RuntimeEvent::SettingDelete(_)) => Ok(()),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Get a reference to the underlying connection.
        pub fn connection(&self) -> Arc<dyn RemoteRuntimeConnection> {
            self.inner.connection.clone()
        }
    }
}
