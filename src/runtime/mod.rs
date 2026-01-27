mod serialization;
#[cfg(feature = "remote")]
pub mod usb;

use alloc::boxed::Box;

use crate::{
    host::Host,
    message::{CommandMessage, EventMessage}
};

pub trait Runtime<InternalCommand = (), InternalEvent = ()>  {
    fn execute_command(&mut self, message: CommandMessage<InternalCommand>) -> Result<EventMessage<InternalEvent>, crate::Error>;
    fn handle_command(&mut self, message: CommandMessage<InternalCommand>) -> Result<(), crate::Error>;
    fn handle_event(&mut self, message: EventMessage<InternalEvent>) -> Result<(), crate::Error>;
    fn host(&self) -> Box<dyn Host>;
}

#[cfg(feature = "remote")]
pub mod remote {
    use std::sync::Arc;
    use async_trait::async_trait;
    use tokio::sync::{Mutex as AsyncMutex, oneshot};
    use crate::{
        Identifier,
        host::remote::RemoteHost,
        message::{CommandMessage, EventMessage, Message}
    };

    /// Trait for providing message transport to a remote runtime.
    ///
    /// This trait abstracts the connection layer, handling only the transport
    /// concerns: connecting, disconnecting, and sending/receiving messages.
    /// The connection is a naive message shuttler with no command/response logic.
    /// Implementations must handle their own internal synchronization.
    #[async_trait]
    pub trait RemoteRuntimeConnection: Send + Sync {
        /// Check if the connection is currently active.
        fn is_connected(&self) -> bool;

        /// Establish a connection to the remote runtime.
        async fn connect(&self) -> Result<(), crate::Error>;

        /// Close the connection to the remote runtime.
        async fn disconnect(&self) -> Result<(), crate::Error>;

        /// Send a message to the remote runtime.
        async fn send_message(&self, message: Message<(), ()>) -> Result<(), crate::Error>;

        /// Receive the next message from the remote runtime.
        /// This should block until a message is available or the connection is closed.
        async fn recv_message(&self) -> Result<Message<(), ()>, crate::Error>;
    }

    /// Registration for a pending command awaiting its response.
    struct CommandResponseRegistration {
        context: Identifier,
        response_tx: Option<oneshot::Sender<EventMessage<()>>>,
    }

    /// Shared internal state for RemoteRuntime.
    /// This is wrapped in Arc so that all clones share the same state.
    struct RemoteRuntimeInner {
        connection: Arc<dyn RemoteRuntimeConnection>,
        pending_commands: AsyncMutex<Vec<CommandResponseRegistration>>,
        /// Channel for messages that aren't command responses (e.g., unsolicited events)
        message_tx: tokio::sync::mpsc::Sender<Message<(), ()>>,
        message_rx: AsyncMutex<tokio::sync::mpsc::Receiver<Message<(), ()>>>,
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
    #[derive(Clone)]
    pub struct RemoteRuntime {
        inner: Arc<RemoteRuntimeInner>,
    }

    impl RemoteRuntime {
        /// Create a new RemoteRuntime wrapping the given connection.
        /// This spawns a background task to receive messages and route responses.
        pub fn new(connection: Arc<dyn RemoteRuntimeConnection>) -> Self {
            let (message_tx, message_rx) = tokio::sync::mpsc::channel(64);

            let inner = Arc::new(RemoteRuntimeInner {
                connection: connection.clone(),
                pending_commands: AsyncMutex::new(Vec::new()),
                message_tx,
                message_rx: AsyncMutex::new(message_rx),
            });

            // Spawn background task to receive and dispatch messages
            let dispatch_inner = inner.clone();
            tokio::spawn(async move {
                Self::message_dispatch_task(dispatch_inner).await;
            });

            Self { inner }
        }

        /// Background task that receives messages from the connection and dispatches them.
        /// Command responses are matched to pending registrations; other messages are
        /// forwarded to the message channel.
        async fn message_dispatch_task(inner: Arc<RemoteRuntimeInner>) {
            loop {
                let message = match inner.connection.recv_message().await {
                    Ok(msg) => msg,
                    Err(e) => {
                        log::trace!("Message dispatch task ending: {:?}", e);
                        break;
                    }
                };

                // Check if this is a response to a pending command
                if let Message::Event(ref event) = message {
                    if let Some(context) = &event.context {
                        let mut pending = inner.pending_commands.lock().await;

                        // Find and fulfill matching registration
                        let mut matched = false;
                        for registration in pending.iter_mut() {
                            if registration.context == *context {
                                if let Some(tx) = registration.response_tx.take() {
                                    let _ = tx.send(event.clone());
                                    matched = true;
                                }
                                break;
                            }
                        }

                        // Clean up fulfilled registrations
                        pending.retain(|r| r.response_tx.is_some());

                        if matched {
                            continue; // Don't forward matched responses to message channel
                        }
                    }
                }

                // Forward unmatched messages to the channel
                if let Err(_) = inner.message_tx.send(message).await {
                    log::trace!("Message channel closed, dispatch task ending");
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
            self.inner.connection.connect().await
        }

        /// Close the connection to the remote runtime.
        pub async fn disconnect(&self) -> Result<(), crate::Error> {
            self.inner.connection.disconnect().await
        }

        /// Execute a command and wait for its response event.
        ///
        /// This method sends a command and blocks until the corresponding
        /// response event is received, matching by context identifier.
        pub async fn execute_command(&self, command: CommandMessage<()>) -> Result<EventMessage<()>, crate::Error> {
            let context = command.identifier.clone();
            let (response_tx, response_rx) = oneshot::channel();

            // Register for the response
            {
                let mut pending = self.inner.pending_commands.lock().await;
                pending.push(CommandResponseRegistration {
                    context,
                    response_tx: Some(response_tx),
                });
            }

            // Send the command
            let message = Message::Command(command);
            self.inner.connection.send_message(message).await?;

            // Wait for the response
            response_rx.await
                .map_err(|_| crate::Error::Debug("command response channel closed".to_string()))
        }

        /// Send a command message (fire-and-forget).
        pub async fn send_command(&self, command: CommandMessage<()>) -> Result<(), crate::Error> {
            let message = Message::Command(command);
            self.inner.connection.send_message(message).await
        }

        /// Send an event message.
        pub async fn send_event(&self, event: EventMessage<()>) -> Result<(), crate::Error> {
            let message = Message::Event(event);
            self.inner.connection.send_message(message).await
        }

        /// Receive the next unmatched message from the remote runtime.
        /// This returns messages that weren't matched as command responses.
        pub async fn next_message(&self) -> Result<Message<(), ()>, crate::Error> {
            let mut rx = self.inner.message_rx.lock().await;
            match rx.recv().await {
                Some(message) => Ok(message),
                None => Err(crate::Error::Busy)
            }
        }

        /// Create a RemoteHost by querying the device for its host information.
        pub async fn host(&self) -> Result<RemoteHost, crate::Error> {
            RemoteHost::from_runtime(self.clone()).await
        }

        /// Get a reference to the underlying connection.
        pub fn connection(&self) -> &Arc<dyn RemoteRuntimeConnection> {
            &self.inner.connection
        }
    }
}
