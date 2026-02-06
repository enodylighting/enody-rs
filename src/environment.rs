use async_trait::async_trait;

use crate::{runtime::remote::RemoteRuntime, Identifier};

/// Environemnts can be used for discovering existing resources or creating
/// user specificed groups of resources for grouped actions.
pub trait Environment {
    /// Returns the unique identifier for this environment
    fn identifier(&self) -> Identifier;

    /// Returns the list of currently known runtimes in this environment.
    /// Since RemoteRuntime implements Clone with a shared internal connection,
    /// the returned runtimes can be freely cloned and shared.
    fn runtimes(&self) -> Vec<RemoteRuntime>;
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
}
