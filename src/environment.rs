//! Discovery abstractions for collections of remote runtimes.
//!
//! An environment is a discovery surface such as USB or WiFi. Environments
//! return connected [`crate::runtime::remote::RemoteRuntime`] handles and, when
//! they support continuous discovery, emit arrival/removal events.

use async_trait::async_trait;

use crate::{runtime::remote::RemoteRuntime, Identifier};

/// Discovery surface that owns or finds remote runtimes.
pub trait Environment {
    /// Returns the unique identifier for this environment.
    fn identifier(&self) -> Identifier;

    /// Returns the list of currently known runtimes in this environment.
    /// Since RemoteRuntime implements Clone with a shared internal connection,
    /// the returned runtimes can be freely cloned and shared.
    fn runtimes(&self) -> Vec<RemoteRuntime>;
}

/// Runtime lifecycle event emitted by a discovery environment.
#[derive(Debug)]
pub enum EnvironmentRuntimeEvent {
    /// A runtime became reachable and was connected.
    Arrived(RemoteRuntime),
    /// A previously connected runtime left or was excluded.
    Left(RemoteRuntime),
}

/// Extension trait for environments that support device discovery.
///
/// This trait provides methods for scanning, starting continuous discovery,
/// and stopping discovery of devices.
#[async_trait(?Send)]
pub trait DiscoveryEnvironment: Environment {
    /// Start continuous discovery in background (hotplug monitoring, mDNS listening, etc.)
    async fn start_discovery(&mut self) -> Result<(), crate::Error>;

    /// Stop continuous discovery
    async fn stop_discovery(&mut self) -> Result<(), crate::Error>;

    /// Wait for the next runtime arrival or removal event.
    async fn next_runtime_event(&self) -> Result<EnvironmentRuntimeEvent, crate::Error>;
}
