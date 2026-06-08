//! WiFi wire protocol and host-side WiFi discovery/authentication.
//!
//! The always-available types in this module are postcard-serialized request
//! and response frames used by the WiFi API. With the `remote` feature enabled,
//! the module also exports `WifiEnvironment`, `WifiConnection`, and
//! `WifiDiscoveredDevice` for mDNS discovery, authenticated Noise transport,
//! and WiFi token pairing.

use crate::message::TOKEN_STRING_MAX_LEN;
use heapless::{String as HString, Vec as HVec};
use serde::{Deserialize, Serialize};

/// Default TCP port for the WiFi API.
pub const WIFI_PORT: u16 = 8788;
/// Supported WiFi API version.
pub const WIFI_API_VERSION: u8 = 1;
/// Maximum plaintext SDK message size carried inside a WiFi frame.
pub const WIFI_MESSAGE_MAX_LEN: usize = 2048;
/// Maximum encrypted Noise payload size carried inside a WiFi frame.
pub const WIFI_FRAME_PAYLOAD_MAX_LEN: usize = WIFI_MESSAGE_MAX_LEN + NOISE_TAG_LEN;
/// mDNS TXT protocol value for compatible devices.
pub const WIFI_PROTOCOL: &str = "enody-v1";
/// mDNS TXT auth value for PSK-authenticated WiFi control.
pub const WIFI_AUTH: &str = "noise-psk";
/// Noise pattern used for authenticated WiFi control.
pub const WIFI_NOISE: &str = "Noise_NNpsk0_25519_ChaChaPoly_SHA256";
/// Noise pattern used for unauthenticated token pairing.
pub const WIFI_PAIRING_NOISE: &str = "Noise_NN_25519_ChaChaPoly_SHA256";

const NOISE_TAG_LEN: usize = 16;

/// Error returned by the WiFi API before or during encrypted transport.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum WifiError {
    /// Request could not be decoded or was malformed.
    BadRequest,
    /// Token or Noise authentication failed.
    Unauthorized,
    /// API version is not supported.
    UnsupportedVersion,
    /// Runtime rejected the request.
    Runtime,
}

/// Client request frame for the WiFi API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Request {
    /// Pairing Noise handshake or encrypted pairing payload.
    PairingNoise {
        /// Requested API version.
        version: u8,
        /// Noise handshake or transport payload.
        payload: HVec<u8, WIFI_FRAME_PAYLOAD_MAX_LEN>,
    },
    /// Authenticated-control hello containing the token key identifier.
    Hello {
        /// Requested API version.
        version: u8,
        /// Token key identifier.
        key_id: HString<TOKEN_STRING_MAX_LEN>,
    },
    /// Authenticated encrypted SDK message payload.
    Noise {
        /// Noise transport payload.
        payload: HVec<u8, WIFI_FRAME_PAYLOAD_MAX_LEN>,
    },
}

/// Device response frame for the WiFi API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Response {
    /// Pairing Noise handshake or encrypted pairing payload.
    PairingNoise {
        /// Noise handshake or transport payload.
        payload: HVec<u8, WIFI_FRAME_PAYLOAD_MAX_LEN>,
    },
    /// Authenticated encrypted SDK message payload.
    Noise {
        /// Noise transport payload.
        payload: HVec<u8, WIFI_FRAME_PAYLOAD_MAX_LEN>,
    },
    /// WiFi API error.
    Error(WifiError),
}

#[cfg(feature = "remote")]
pub use remote::{WifiConnection, WifiDiscoveredDevice, WifiEnvironment};

#[cfg(feature = "remote")]
mod remote {
    use super::*;
    use crate::{
        environment::{DiscoveryEnvironment, Environment, EnvironmentRuntimeEvent},
        message::{Event, Message, RuntimeEvent, Token},
        runtime::remote::{RemoteRuntime, RemoteRuntimeConnection},
        Identifier,
    };
    use async_trait::async_trait;
    use edge_mdns::{
        domain::{
            base::{
                iana::{Class, Rtype},
                name::ToName,
                Message as MdnsMessage, Question,
            },
            rdata::AllRecordData,
        },
        HostQuestions, MdnsError, NameSlice, PeerAnswer,
    };
    use serde::de::DeserializeOwned;
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    use std::{
        collections::{HashMap, HashSet},
        fmt,
        net::{Ipv4Addr, SocketAddrV4},
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use tokio::{
        io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
        net::{tcp::OwnedWriteHalf, TcpStream, UdpSocket},
        sync::{mpsc, Mutex as AsyncMutex, RwLock},
        task::JoinHandle,
    };

    const POSTCARD_FRAME_OVERHEAD_LEN: usize = 8;
    const FRAME_MAX_LEN: usize = WIFI_FRAME_PAYLOAD_MAX_LEN + POSTCARD_FRAME_OVERHEAD_LEN;
    const DEFAULT_WIFI_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(800);
    const WIFI_DISCOVERY_POLL_INTERVAL: Duration = Duration::from_secs(2);
    const WIFI_DISCOVERY_MISSES_BEFORE_LEFT: u8 = 3;
    const WIFI_RUNTIME_EVENT_CHANNEL_SIZE: usize = 16;
    const WIFI_SERVICE_LABELS: &[&str] = &["_enody", "_tcp", "local"];
    const WIFI_SERVICE_SUFFIX: &str = "._enody._tcp.local";
    const MDNS_MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
    const MDNS_PORT: u16 = 5353;
    const NOISE_PROLOGUE_LABEL: &[u8] = b"enody-v1 noise";
    const PAIRING_NOISE_PROLOGUE_LABEL: &[u8] = b"enody-v1 pairing";
    const PAIRING_TOKEN_TIMEOUT: Duration = Duration::from_secs(45);

    #[derive(Clone, Debug)]
    struct ConnectedWifiRuntime {
        runtime: RemoteRuntime,
        missed_discoveries: u8,
    }

    /// WiFi-based environment for discovering and managing WiFi-connected Enody devices.
    pub struct WifiEnvironment {
        identifier: Identifier,
        tokens: Arc<HashMap<Identifier, Token>>,
        excluded_host_ids: Arc<Mutex<HashSet<Identifier>>>,
        connected_runtimes: Arc<Mutex<Vec<ConnectedWifiRuntime>>>,
        runtime_event_tx: mpsc::Sender<EnvironmentRuntimeEvent>,
        runtime_event_rx: AsyncMutex<mpsc::Receiver<EnvironmentRuntimeEvent>>,
        discovery_task: Option<JoinHandle<()>>,
        discovery_timeout: Duration,
        /// Cached runtime handle for use during Drop (when the tokio context may
        /// not be active, e.g. when called from Python's garbage collector).
        runtime_handle: Option<tokio::runtime::Handle>,
    }

    impl WifiEnvironment {
        /// Create a new WiFi environment using saved authentication tokens.
        ///
        /// Discovery is performed immediately using mDNS, and only devices with a
        /// matching token and supported WiFi metadata are connected.
        pub async fn new(tokens: impl IntoIterator<Item = Token>) -> Result<Self, crate::Error> {
            Self::with_excluded_host_ids(tokens, std::iter::empty()).await
        }

        /// Create a new WiFi environment while skipping host identifiers already
        /// represented by another environment, such as USB.
        pub async fn with_excluded_host_ids<T, H>(
            tokens: T,
            excluded_host_ids: H,
        ) -> Result<Self, crate::Error>
        where
            T: IntoIterator<Item = Token>,
            H: IntoIterator<Item = Identifier>,
        {
            Self::with_timeout_and_excluded_host_ids(
                tokens,
                DEFAULT_WIFI_DISCOVERY_TIMEOUT,
                excluded_host_ids,
            )
            .await
        }

        /// Create a new WiFi environment with a custom mDNS discovery timeout.
        pub async fn with_timeout<T>(tokens: T, timeout: Duration) -> Result<Self, crate::Error>
        where
            T: IntoIterator<Item = Token>,
        {
            Self::with_timeout_and_excluded_host_ids(tokens, timeout, std::iter::empty()).await
        }

        /// Create a new WiFi environment with a custom mDNS discovery timeout while
        /// skipping host identifiers already represented by another environment.
        pub async fn with_timeout_and_excluded_host_ids<T, H>(
            tokens: T,
            timeout: Duration,
            excluded_host_ids: H,
        ) -> Result<Self, crate::Error>
        where
            T: IntoIterator<Item = Token>,
            H: IntoIterator<Item = Identifier>,
        {
            let tokens: HashMap<Identifier, Token> = tokens
                .into_iter()
                .map(|token| (token.host_id, token))
                .collect();
            let excluded_host_ids: HashSet<Identifier> = excluded_host_ids.into_iter().collect();
            let mut connected_runtimes = Vec::new();

            if tokens.is_empty() {
                log::debug!(
                    "Skipping WiFi mDNS discovery because no saved WiFi tokens are available"
                );
            } else {
                log::debug!(
                    "Discovering WiFi runtimes over mDNS using {} saved token(s)",
                    tokens.len()
                );
                let devices = discover(timeout).await?;
                let mut seen_host_ids = HashSet::new();

                for device in devices {
                    let Some(host_id) = device.host_id else {
                        continue;
                    };
                    if excluded_host_ids.contains(&host_id)
                        || Self::token_for_discovered_device(&tokens, &device).is_none()
                        || !seen_host_ids.insert(host_id)
                    {
                        continue;
                    }

                    let Some((_, runtime)) =
                        Self::runtime_from_discovered_device(&tokens, &excluded_host_ids, &device)
                            .await
                    else {
                        continue;
                    };
                    connected_runtimes.push(ConnectedWifiRuntime {
                        runtime,
                        missed_discoveries: 0,
                    });
                }
            }

            let (runtime_event_tx, runtime_event_rx) =
                mpsc::channel::<EnvironmentRuntimeEvent>(WIFI_RUNTIME_EVENT_CHANNEL_SIZE);
            Ok(Self {
                identifier: Identifier::new_v4(),
                tokens: Arc::new(tokens),
                excluded_host_ids: Arc::new(Mutex::new(excluded_host_ids)),
                connected_runtimes: Arc::new(Mutex::new(connected_runtimes)),
                runtime_event_tx,
                runtime_event_rx: AsyncMutex::new(runtime_event_rx),
                discovery_task: None,
                discovery_timeout: timeout,
                runtime_handle: tokio::runtime::Handle::try_current().ok(),
            })
        }

        fn runtime_host_id(runtime: &RemoteRuntime) -> Identifier {
            runtime.connection().identifier()
        }

        fn remove_connected_runtime(
            connected_runtimes: &Arc<Mutex<Vec<ConnectedWifiRuntime>>>,
            host_id: Identifier,
        ) -> Option<RemoteRuntime> {
            let mut connected_guard = connected_runtimes.lock().unwrap();
            let index = connected_guard
                .iter()
                .position(|connected| Self::runtime_host_id(&connected.runtime) == host_id)?;
            Some(connected_guard.remove(index).runtime)
        }

        /// Exclude and disconnect a host that is already represented elsewhere.
        ///
        /// This is useful when USB discovery has found the same product and the
        /// application wants to avoid duplicate runtimes.
        pub async fn exclude_host_id(&self, host_id: Identifier) {
            let inserted = {
                let mut excluded_guard = self.excluded_host_ids.lock().unwrap();
                excluded_guard.insert(host_id)
            };
            if !inserted {
                return;
            }

            let Some(runtime) = Self::remove_connected_runtime(&self.connected_runtimes, host_id)
            else {
                return;
            };

            if runtime.is_connected() {
                if let Err(error) = runtime.disconnect().await {
                    log::warn!(
                        "Failed to disconnect excluded WiFi runtime {}: {:?}",
                        Self::runtime_host_id(&runtime),
                        error
                    );
                }
            }

            match self
                .runtime_event_tx
                .try_send(EnvironmentRuntimeEvent::Left(runtime))
            {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    log::trace!("WiFi runtime event channel full, dropping exclusion Left event");
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    log::trace!("WiFi runtime event channel closed");
                }
            }
        }

        /// Remove a host identifier from the exclusion list.
        pub fn remove_excluded_host_id(&self, host_id: Identifier) {
            let mut excluded_guard = self.excluded_host_ids.lock().unwrap();
            excluded_guard.remove(&host_id);
        }

        async fn runtime_from_discovered_device(
            tokens: &HashMap<Identifier, Token>,
            excluded_host_ids: &HashSet<Identifier>,
            device: &WifiDiscoveredDevice,
        ) -> Option<(Identifier, RemoteRuntime)> {
            let Some(host_id) = device.host_id else {
                return None;
            };
            if excluded_host_ids.contains(&host_id) {
                return None;
            }

            let Some(token) = Self::token_for_discovered_device(tokens, device) else {
                return None;
            };
            let runtime = match WifiConnection::runtime_from_discovered_device(token, device) {
                Ok(runtime) => runtime,
                Err(error) => {
                    log::debug!(
                        "Ignoring discovered WiFi runtime {host_id} with invalid saved token: {error:?}",
                    );
                    return None;
                }
            };
            match runtime.connect().await {
                Ok(()) => Some((host_id, runtime)),
                Err(error) => {
                    log::debug!("Failed to connect to WiFi runtime {}: {:?}", host_id, error);
                    None
                }
            }
        }

        async fn reconcile_discovered_devices(
            connected_runtimes: &Arc<Mutex<Vec<ConnectedWifiRuntime>>>,
            tokens: &HashMap<Identifier, Token>,
            excluded_host_ids: &HashSet<Identifier>,
            devices: Vec<WifiDiscoveredDevice>,
        ) -> Vec<EnvironmentRuntimeEvent> {
            let mut events = Vec::new();
            let mut seen_host_ids = HashSet::new();

            for device in devices {
                let Some(host_id) = device.host_id else {
                    continue;
                };
                if excluded_host_ids.contains(&host_id)
                    || Self::token_for_discovered_device(tokens, &device).is_none()
                    || !seen_host_ids.insert(host_id)
                {
                    continue;
                }

                let already_connected = {
                    let mut connected_guard = connected_runtimes.lock().unwrap();
                    if let Some(connected) = connected_guard
                        .iter_mut()
                        .find(|connected| Self::runtime_host_id(&connected.runtime) == host_id)
                    {
                        connected.missed_discoveries = 0;
                        true
                    } else {
                        false
                    }
                };
                if already_connected {
                    continue;
                }

                let Some((_, runtime)) =
                    Self::runtime_from_discovered_device(tokens, excluded_host_ids, &device).await
                else {
                    continue;
                };

                {
                    let mut connected_guard = connected_runtimes.lock().unwrap();
                    if connected_guard
                        .iter()
                        .any(|connected| Self::runtime_host_id(&connected.runtime) == host_id)
                    {
                        continue;
                    }
                    connected_guard.push(ConnectedWifiRuntime {
                        runtime: runtime.clone(),
                        missed_discoveries: 0,
                    });
                }
                events.push(EnvironmentRuntimeEvent::Arrived(runtime));
            }

            let missing_runtimes = {
                let mut connected_guard = connected_runtimes.lock().unwrap();
                let mut missing_runtimes = Vec::new();
                let mut index = 0;
                while index < connected_guard.len() {
                    let host_id = Self::runtime_host_id(&connected_guard[index].runtime);
                    if seen_host_ids.contains(&host_id) {
                        connected_guard[index].missed_discoveries = 0;
                        index += 1;
                        continue;
                    }

                    connected_guard[index].missed_discoveries =
                        connected_guard[index].missed_discoveries.saturating_add(1);
                    if connected_guard[index].missed_discoveries
                        >= WIFI_DISCOVERY_MISSES_BEFORE_LEFT
                    {
                        missing_runtimes.push(connected_guard.remove(index).runtime);
                    } else {
                        index += 1;
                    }
                }
                missing_runtimes
            };

            for runtime in missing_runtimes {
                if runtime.is_connected() {
                    if let Err(error) = runtime.disconnect().await {
                        log::warn!(
                            "Failed to disconnect missing WiFi runtime {}: {:?}",
                            Self::runtime_host_id(&runtime),
                            error
                        );
                    }
                }
                events.push(EnvironmentRuntimeEvent::Left(runtime));
            }

            events
        }

        fn token_for_discovered_device<'a>(
            tokens: &'a HashMap<Identifier, Token>,
            device: &WifiDiscoveredDevice,
        ) -> Option<&'a Token> {
            let Some(host_id) = device.host_id else {
                return None;
            };
            let Some(stored) = tokens.get(&host_id) else {
                log::debug!(
                    "Ignoring discovered WiFi runtime {} without saved token",
                    host_id
                );
                return None;
            };
            if device.protocol.as_deref() != Some(WIFI_PROTOCOL) {
                log::debug!(
                    "Ignoring discovered WiFi runtime {} with unsupported protocol {:?}",
                    host_id,
                    device.protocol
                );
                return None;
            }
            if device.auth.as_deref() != Some(WIFI_AUTH) {
                log::debug!(
                    "Ignoring discovered WiFi runtime {} with unsupported auth {:?}",
                    host_id,
                    device.auth
                );
                return None;
            }

            Some(stored)
        }

        fn disconnect_runtime_with_handle(
            runtime: &RemoteRuntime,
            handle: &tokio::runtime::Handle,
        ) -> Result<(), crate::Error> {
            let handle = handle.clone();
            let runtime = runtime.clone();
            std::thread::spawn(move || handle.block_on(runtime.disconnect()))
                .join()
                .map_err(|_| crate::Error::Debug("disconnect thread panicked".to_string()))?
        }
    }

    impl Environment for WifiEnvironment {
        fn identifier(&self) -> Identifier {
            self.identifier
        }

        fn runtimes(&self) -> Vec<RemoteRuntime> {
            let connected_guard = self.connected_runtimes.lock().unwrap();
            connected_guard
                .iter()
                .map(|connected| connected.runtime.clone())
                .collect()
        }
    }

    #[async_trait(?Send)]
    impl DiscoveryEnvironment for WifiEnvironment {
        async fn start_discovery(&mut self) -> Result<(), crate::Error> {
            if self.discovery_task.is_some() {
                return Ok(());
            }

            let task_connected_runtimes = Arc::clone(&self.connected_runtimes);
            let task_tokens = Arc::clone(&self.tokens);
            let task_excluded_host_ids = Arc::clone(&self.excluded_host_ids);
            let task_runtime_event_tx = self.runtime_event_tx.clone();
            let discovery_timeout = self.discovery_timeout;

            self.discovery_task = Some(tokio::spawn(async move {
                let mut interval = tokio::time::interval(WIFI_DISCOVERY_POLL_INTERVAL);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    interval.tick().await;
                    let devices = match discover(discovery_timeout).await {
                        Ok(devices) => devices,
                        Err(error) => {
                            log::debug!("WiFi mDNS discovery failed: {:?}", error);
                            continue;
                        }
                    };

                    let excluded_host_ids = task_excluded_host_ids.lock().unwrap().clone();
                    let events = Self::reconcile_discovered_devices(
                        &task_connected_runtimes,
                        task_tokens.as_ref(),
                        &excluded_host_ids,
                        devices,
                    )
                    .await;

                    for event in events {
                        if task_runtime_event_tx.send(event).await.is_err() {
                            return;
                        }
                    }
                }
            }));

            Ok(())
        }

        async fn stop_discovery(&mut self) -> Result<(), crate::Error> {
            if let Some(task) = self.discovery_task.take() {
                task.abort();
                let _ = task.await;
            }
            Ok(())
        }

        async fn next_runtime_event(&self) -> Result<EnvironmentRuntimeEvent, crate::Error> {
            if self.discovery_task.is_none() {
                return Err(crate::Error::Debug(
                    "discovery is not running; call start_discovery first".to_string(),
                ));
            }

            let mut runtime_event_rx = self.runtime_event_rx.lock().await;
            runtime_event_rx.recv().await.ok_or_else(|| {
                crate::Error::Debug("runtime event stream closed unexpectedly".to_string())
            })
        }
    }

    impl Drop for WifiEnvironment {
        fn drop(&mut self) {
            if let Some(task) = self.discovery_task.take() {
                task.abort();
            }

            let runtimes: Vec<RemoteRuntime> = {
                let connected_guard = self.connected_runtimes.lock().unwrap();
                connected_guard
                    .iter()
                    .map(|connected| connected.runtime.clone())
                    .collect()
            };

            let handle = tokio::runtime::Handle::try_current()
                .ok()
                .or_else(|| self.runtime_handle.clone());

            for runtime in runtimes {
                if !runtime.is_connected() {
                    continue;
                }

                let result = match &handle {
                    Some(h) => Self::disconnect_runtime_with_handle(&runtime, h),
                    None => {
                        log::error!(
                            "No tokio runtime available to disconnect WiFi runtime on drop"
                        );
                        continue;
                    }
                };

                if let Err(e) = result {
                    log::error!(
                        "Failed to disconnect runtime on WifiEnvironment drop: {:?}",
                        e
                    );
                }
            }
        }
    }

    #[derive(Debug)]
    struct ConnectionState {
        writer: Arc<AsyncMutex<OwnedWriteHalf>>,
        noise: Arc<AsyncMutex<snow::TransportState>>,
        read_task: JoinHandle<Result<(), crate::Error>>,
    }

    /// Authenticated TCP/Noise connection to one WiFi runtime.
    ///
    /// Construct this from a saved [`Token`](crate::message::Token), then wrap
    /// it in [`RemoteRuntime`] or use the `runtime_from_*` constructors.
    pub struct WifiConnection {
        endpoint: String,
        identifier: Identifier,
        key_id: HString<TOKEN_STRING_MAX_LEN>,
        key: [u8; 32],
        connection: RwLock<Option<ConnectionState>>,
        message_rx: AsyncMutex<Option<mpsc::Receiver<Message>>>,
    }

    impl fmt::Debug for WifiConnection {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("WifiConnection")
                .field("endpoint", &self.endpoint)
                .field("identifier", &self.identifier)
                .finish_non_exhaustive()
        }
    }

    impl WifiConnection {
        /// Creates a WiFi connection for a known endpoint such as `"host:8788"`.
        pub fn from_endpoint(
            token: &Token,
            endpoint: impl Into<String>,
        ) -> Result<Self, crate::Error> {
            let key: [u8; 32] = token
                .data
                .as_slice()
                .try_into()
                .map_err(|_| crate::Error::Argument)?;
            Ok(Self {
                endpoint: endpoint.into(),
                identifier: token.host_id,
                key_id: token.key_id.clone(),
                key,
                connection: RwLock::new(None),
                message_rx: AsyncMutex::new(None),
            })
        }

        /// Creates a WiFi connection from an mDNS-discovered device.
        pub fn from_discovered_device(
            token: &Token,
            device: &WifiDiscoveredDevice,
        ) -> Result<Self, crate::Error> {
            let endpoint = device.endpoint().ok_or(crate::Error::Argument)?;
            Self::from_endpoint(token, endpoint)
        }

        /// Creates a remote runtime for a known WiFi endpoint.
        pub fn runtime_from_endpoint(
            token: &Token,
            endpoint: impl Into<String>,
        ) -> Result<RemoteRuntime, crate::Error> {
            Ok(RemoteRuntime::new(Box::new(Self::from_endpoint(
                token, endpoint,
            )?)))
        }

        /// Creates a remote runtime from an mDNS-discovered device.
        pub fn runtime_from_discovered_device(
            token: &Token,
            device: &WifiDiscoveredDevice,
        ) -> Result<RemoteRuntime, crate::Error> {
            Ok(RemoteRuntime::new(Box::new(Self::from_discovered_device(
                token, device,
            )?)))
        }

        /// Generate a new WiFi token through the unauthenticated generation flow.
        ///
        /// The device will ask for physical approval before issuing the token.
        /// `on_approval` is called when the device sends its approval
        /// instruction, usually so a UI can show the user what gesture to
        /// perform.
        pub async fn generate_token_from_endpoint_with_approval<F>(
            endpoint: impl Into<String>,
            on_approval: F,
        ) -> Result<Token, crate::Error>
        where
            F: FnMut(&str),
        {
            generate_token_from_endpoint(endpoint, on_approval).await
        }

        /// Generate a new WiFi token for a discovered device.
        pub async fn generate_token_from_discovered_device_with_approval<F>(
            device: &WifiDiscoveredDevice,
            on_approval: F,
        ) -> Result<Token, crate::Error>
        where
            F: FnMut(&str),
        {
            let endpoint = device.endpoint().ok_or(crate::Error::Argument)?;
            Self::generate_token_from_endpoint_with_approval(endpoint, on_approval).await
        }

        /// Discover EP01 devices that advertise enough mDNS metadata to attempt
        /// WiFi token generation.
        pub async fn discover_token_generation_devices(
            timeout: Duration,
        ) -> Result<Vec<WifiDiscoveredDevice>, crate::Error> {
            let mut devices = discover(timeout).await?;
            devices.retain(WifiDiscoveredDevice::is_token_generation_candidate);
            devices.sort_by_key(|device| device.endpoint());
            Ok(devices)
        }

        async fn authenticate(
            &self,
            stream: &mut TcpStream,
        ) -> Result<snow::TransportState, crate::Error> {
            write_frame(
                stream,
                &Request::Hello {
                    version: WIFI_API_VERSION,
                    key_id: self.key_id.clone(),
                },
            )
            .await?;

            let mut noise =
                build_noise_initiator(&self.key, self.key_id.as_str(), self.identifier)?;
            let mut handshake = [0u8; WIFI_FRAME_PAYLOAD_MAX_LEN];
            let handshake_len = noise
                .write_message(&[], &mut handshake)
                .map_err(noise_error)?;
            write_frame(stream, &client_noise_frame(&handshake[..handshake_len])?).await?;

            match read_frame::<_, Response>(stream).await? {
                Response::Noise { payload } => {
                    let mut empty_payload = [0u8; 0];
                    let len = noise
                        .read_message(payload.as_slice(), &mut empty_payload)
                        .map_err(noise_error)?;
                    if len != 0 {
                        return Err(crate::Error::UnexpectedResponse);
                    }
                    noise.into_transport_mode().map_err(noise_error)
                }
                Response::PairingNoise { .. } => Err(crate::Error::UnexpectedResponse),
                Response::Error(error) => Err(wifi_error(error)),
            }
        }
    }

    #[async_trait]
    impl RemoteRuntimeConnection for WifiConnection {
        fn identifier(&self) -> Identifier {
            self.identifier
        }

        fn is_connected(&self) -> bool {
            self.connection
                .try_read()
                .map(|connection| connection.is_some())
                .unwrap_or(false)
        }

        async fn connect(&self) -> Result<(), crate::Error> {
            let mut connection = self.connection.write().await;
            if connection.is_some() {
                return Ok(());
            }

            let mut stream = TcpStream::connect(&self.endpoint)
                .await
                .map_err(|error| crate::Error::Debug(error.to_string()))?;
            stream
                .set_nodelay(true)
                .map_err(|error| crate::Error::Debug(error.to_string()))?;
            let noise = self.authenticate(&mut stream).await?;

            let (read_half, write_half) = stream.into_split();
            let (message_tx, message_rx) = mpsc::channel(64);
            let noise = Arc::new(AsyncMutex::new(noise));
            let read_noise = noise.clone();
            let read_task =
                tokio::spawn(async move { read_task(read_half, message_tx, read_noise).await });

            *connection = Some(ConnectionState {
                writer: Arc::new(AsyncMutex::new(write_half)),
                noise,
                read_task,
            });

            drop(connection);
            let mut rx = self.message_rx.lock().await;
            *rx = Some(message_rx);
            Ok(())
        }

        async fn disconnect(&self) -> Result<(), crate::Error> {
            let connection = {
                let mut connection = self.connection.write().await;
                connection.take()
            };
            if let Some(connection) = connection {
                connection.read_task.abort();
                let _ = connection.read_task.await;
            }

            let mut rx = self.message_rx.lock().await;
            *rx = None;
            Ok(())
        }

        async fn send_message(&self, message: Message) -> Result<(), crate::Error> {
            let (writer, noise) = {
                let connection = self.connection.read().await;
                let Some(connection) = connection.as_ref() else {
                    return Err(crate::Error::Busy);
                };
                (connection.writer.clone(), connection.noise.clone())
            };

            let payload =
                postcard::to_allocvec(&message).map_err(|_| crate::Error::Serialization)?;
            if payload.len() > WIFI_MESSAGE_MAX_LEN {
                return Err(crate::Error::Serialization);
            }
            let mut encrypted = [0u8; WIFI_FRAME_PAYLOAD_MAX_LEN];
            let encrypted_len = {
                let mut noise = noise.lock().await;
                noise
                    .write_message(&payload, &mut encrypted)
                    .map_err(noise_error)?
            };
            let frame = client_noise_frame(&encrypted[..encrypted_len])?;
            let mut writer = writer.lock().await;
            write_frame(&mut *writer, &frame).await
        }

        async fn recv_message(&self) -> Result<Message, crate::Error> {
            let mut rx = self.message_rx.lock().await;
            let Some(rx) = rx.as_mut() else {
                return Err(crate::Error::Busy);
            };
            rx.recv().await.ok_or(crate::Error::Timeout)
        }
    }

    async fn read_task<R>(
        mut reader: R,
        message_tx: mpsc::Sender<Message>,
        noise: Arc<AsyncMutex<snow::TransportState>>,
    ) -> Result<(), crate::Error>
    where
        R: AsyncRead + Unpin,
    {
        loop {
            let frame = read_frame::<_, Response>(&mut reader).await?;
            match frame {
                Response::Noise { payload } => {
                    let mut decrypted = [0u8; WIFI_MESSAGE_MAX_LEN];
                    let decrypted_len = {
                        let mut noise = noise.lock().await;
                        noise
                            .read_message(payload.as_slice(), &mut decrypted)
                            .map_err(noise_error)?
                    };
                    let message: Message = postcard::from_bytes(&decrypted[..decrypted_len])
                        .map_err(|_| crate::Error::Serialization)?;
                    if message_tx.send(message).await.is_err() {
                        return Ok(());
                    }
                }
                Response::PairingNoise { .. } => return Err(crate::Error::UnexpectedResponse),
                Response::Error(error) => return Err(wifi_error(error)),
            }
        }
    }

    async fn read_frame<R, T>(reader: &mut R) -> Result<T, crate::Error>
    where
        R: AsyncRead + Unpin,
        T: DeserializeOwned,
    {
        let mut len_bytes = [0u8; 2];
        reader
            .read_exact(&mut len_bytes)
            .await
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        let len = u16::from_be_bytes(len_bytes) as usize;
        if len == 0 || len > FRAME_MAX_LEN {
            return Err(crate::Error::InsufficientData);
        }

        let mut payload = vec![0u8; len];
        reader
            .read_exact(&mut payload)
            .await
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        postcard::from_bytes(&payload).map_err(|_| crate::Error::Serialization)
    }

    async fn write_frame<W, T>(writer: &mut W, frame: &T) -> Result<(), crate::Error>
    where
        W: AsyncWrite + Unpin,
        T: Serialize,
    {
        let payload = postcard::to_allocvec(frame).map_err(|_| crate::Error::Serialization)?;
        let len = u16::try_from(payload.len()).map_err(|_| crate::Error::Serialization)?;

        let mut buffer = Vec::with_capacity(2 + payload.len());
        buffer.extend_from_slice(&len.to_be_bytes());
        buffer.extend_from_slice(&payload);

        writer
            .write_all(&buffer)
            .await
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|error| crate::Error::Debug(error.to_string()))
    }

    fn wifi_error(error: WifiError) -> crate::Error {
        match error {
            WifiError::Unauthorized => crate::Error::Permission,
            WifiError::UnsupportedVersion => crate::Error::Unsupported,
            WifiError::BadRequest => crate::Error::Argument,
            WifiError::Runtime => crate::Error::Unknown,
        }
    }

    fn noise_error(error: snow::Error) -> crate::Error {
        match error {
            snow::Error::Decrypt => crate::Error::Permission,
            _ => crate::Error::Debug(format!("noise protocol error: {:?}", error)),
        }
    }

    fn noise_prologue(key_id: &str, host_id: Identifier) -> Result<Vec<u8>, crate::Error> {
        let mut prologue = Vec::with_capacity(NOISE_PROLOGUE_LABEL.len() + key_id.len() + 16);
        prologue.extend_from_slice(NOISE_PROLOGUE_LABEL);
        prologue.extend_from_slice(key_id.as_bytes());
        prologue.extend_from_slice(host_id.as_bytes());
        Ok(prologue)
    }

    fn build_noise_initiator(
        key: &[u8; 32],
        key_id: &str,
        host_id: Identifier,
    ) -> Result<snow::HandshakeState, crate::Error> {
        let params: snow::params::NoiseParams = WIFI_NOISE.parse().map_err(noise_error)?;
        let prologue = noise_prologue(key_id, host_id)?;
        snow::Builder::new(params)
            .psk(0, key)
            .map_err(noise_error)?
            .prologue(&prologue)
            .map_err(noise_error)?
            .build_initiator()
            .map_err(noise_error)
    }

    fn pairing_noise_prologue() -> Vec<u8> {
        PAIRING_NOISE_PROLOGUE_LABEL.to_vec()
    }

    fn build_pairing_noise_initiator() -> Result<snow::HandshakeState, crate::Error> {
        let params: snow::params::NoiseParams = WIFI_PAIRING_NOISE.parse().map_err(noise_error)?;
        let prologue = pairing_noise_prologue();
        snow::Builder::new(params)
            .prologue(&prologue)
            .map_err(noise_error)?
            .build_initiator()
            .map_err(noise_error)
    }

    fn client_noise_frame(payload: &[u8]) -> Result<Request, crate::Error> {
        let payload = HVec::from_slice(payload).map_err(|_| crate::Error::Serialization)?;
        Ok(Request::Noise { payload })
    }

    fn client_pairing_noise_frame(payload: &[u8]) -> Result<Request, crate::Error> {
        let payload = HVec::from_slice(payload).map_err(|_| crate::Error::Serialization)?;
        Ok(Request::PairingNoise {
            version: WIFI_API_VERSION,
            payload,
        })
    }

    async fn generate_token_from_endpoint<F>(
        endpoint: impl Into<String>,
        mut on_approval: F,
    ) -> Result<Token, crate::Error>
    where
        F: FnMut(&str),
    {
        let endpoint = endpoint.into();
        log::debug!("WiFi token generation pairing connecting to {}", endpoint);
        let mut stream = TcpStream::connect(&endpoint)
            .await
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        stream
            .set_nodelay(true)
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        log::debug!("WiFi token generation pairing connected to {}", endpoint);

        let mut noise = build_pairing_noise_initiator()?;
        let mut handshake = [0u8; WIFI_FRAME_PAYLOAD_MAX_LEN];
        let handshake_len = noise
            .write_message(&[], &mut handshake)
            .map_err(noise_error)?;
        log::debug!(
            "WiFi token generation pairing sending initiator handshake ({} bytes)",
            handshake_len
        );
        write_frame(
            &mut stream,
            &client_pairing_noise_frame(&handshake[..handshake_len])?,
        )
        .await?;

        log::debug!("WiFi token generation pairing waiting for responder handshake");
        match read_frame::<_, Response>(&mut stream).await? {
            Response::PairingNoise { payload } => {
                log::debug!(
                    "WiFi token generation pairing received responder handshake ({} bytes)",
                    payload.len()
                );
                let mut empty_payload = [0u8; 0];
                let len = noise
                    .read_message(payload.as_slice(), &mut empty_payload)
                    .map_err(noise_error)?;
                if len != 0 {
                    return Err(crate::Error::UnexpectedResponse);
                }
            }
            Response::Error(error) => return Err(wifi_error(error)),
            _ => return Err(crate::Error::UnexpectedResponse),
        }

        log::debug!("WiFi token generation pairing transport established");
        let mut transport = noise.into_transport_mode().map_err(noise_error)?;
        let deadline = Instant::now() + PAIRING_TOKEN_TIMEOUT;
        log::debug!(
            "WiFi token generation pairing waiting for approval/token events for up to {:?}",
            PAIRING_TOKEN_TIMEOUT
        );
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let frame =
                match tokio::time::timeout(remaining, read_frame::<_, Response>(&mut stream)).await
                {
                    Ok(frame) => frame?,
                    Err(_) => {
                        log::debug!("WiFi token generation pairing timed out waiting for event");
                        return Err(crate::Error::Timeout);
                    }
                };
            let payload = match frame {
                Response::PairingNoise { payload } => payload,
                Response::Error(error) => return Err(wifi_error(error)),
                _ => return Err(crate::Error::UnexpectedResponse),
            };

            let mut plaintext = [0u8; WIFI_MESSAGE_MAX_LEN];
            let plaintext_len = transport
                .read_message(payload.as_slice(), &mut plaintext)
                .map_err(noise_error)?;
            let message: Message = postcard::from_bytes(&plaintext[..plaintext_len])
                .map_err(|_| crate::Error::Serialization)?;
            let Message::Event(event) = message else {
                return Err(crate::Error::UnexpectedResponse);
            };

            match event.event {
                Event::Runtime(RuntimeEvent::TokenGenerateApproval(method)) => {
                    log::debug!(
                        "WiFi token generation pairing received approval instruction: {}",
                        method.as_str()
                    );
                    on_approval(method.as_str());
                }
                Event::Runtime(RuntimeEvent::TokenGenerated(token)) => {
                    log::debug!(
                        "WiFi token generation pairing received generated token for host {}",
                        token.host_id
                    );
                    return Ok(token);
                }
                Event::Error(error) => {
                    log::debug!(
                        "WiFi token generation pairing received error event: {:?}",
                        error
                    );
                    return Err(error);
                }
                _ => return Err(crate::Error::UnexpectedResponse),
            }
        }

        log::debug!("WiFi token generation pairing deadline elapsed");
        Err(crate::Error::Timeout)
    }

    /// Device advertised over mDNS as `_enody._tcp.local`.
    #[derive(Clone, Debug, Default)]
    pub struct WifiDiscoveredDevice {
        /// mDNS service instance name.
        pub instance: String,
        /// Host name advertised by mDNS.
        pub host: String,
        /// Resolved IPv4 address, when available.
        pub address: Option<Ipv4Addr>,
        /// Product model TXT value.
        pub model: Option<String>,
        /// WiFi API version TXT value.
        pub api_version: Option<u8>,
        /// Host identifier TXT value.
        pub host_id: Option<Identifier>,
        /// Firmware version TXT value.
        pub firmware_version: Option<String>,
        /// Optional HTTP port TXT value.
        pub http_port: Option<u16>,
        /// WiFi API TCP port TXT value.
        pub port: Option<u16>,
        /// Protocol TXT value.
        pub protocol: Option<String>,
        /// Authentication TXT value.
        pub auth: Option<String>,
    }

    impl WifiDiscoveredDevice {
        /// Returns a `host:port` endpoint suitable for [`WifiConnection`].
        pub fn endpoint(&self) -> Option<String> {
            let host = if let Some(address) = self.address {
                address.to_string()
            } else {
                let host = self.host.trim_end_matches('.');
                if host.is_empty() {
                    return None;
                }
                host.to_owned()
            };
            Some(format!("{}:{}", host, self.port.unwrap_or(WIFI_PORT)))
        }

        /// Returns whether this advertisement appears to be an EP01.
        pub fn is_ep01(&self) -> bool {
            self.model
                .as_deref()
                .map(|model| model.eq_ignore_ascii_case("ep01"))
                .unwrap_or_else(|| {
                    self.instance
                        .trim_end_matches('.')
                        .to_ascii_lowercase()
                        .starts_with("ep01 ")
                })
        }

        /// Returns whether this device can be used for WiFi token pairing.
        pub fn is_token_generation_candidate(&self) -> bool {
            self.endpoint().is_some()
                && self.protocol.as_deref() == Some(WIFI_PROTOCOL)
                && self.is_ep01()
        }
    }

    pub async fn discover(timeout: Duration) -> Result<Vec<WifiDiscoveredDevice>, crate::Error> {
        let query_id_bytes = uuid::Uuid::new_v4();
        let query_id =
            u16::from_be_bytes([query_id_bytes.as_bytes()[0], query_id_bytes.as_bytes()[1]]);
        let query = build_mdns_ptr_query(query_id)?;
        let interfaces = mdns_outbound_interfaces();
        let multicast_addr = SocketAddrV4::new(MDNS_MULTICAST_ADDR, MDNS_PORT);
        let mut sockets = Vec::new();
        log::debug!(
            "Sending mDNS query for _enody._tcp.local with timeout {:?} on {} interface(s)",
            timeout,
            interfaces.len()
        );

        for interface in interfaces {
            let socket = match create_mdns_socket(interface) {
                Ok(socket) => socket,
                Err(error) => {
                    log::debug!(
                        "Failed to create mDNS socket for interface {}: {:?}",
                        interface,
                        error
                    );
                    continue;
                }
            };

            match socket.send_to(&query, multicast_addr).await {
                Ok(len) => {
                    log::trace!(
                        "Sent {} byte mDNS PTR query on interface {}",
                        len,
                        interface
                    );
                    sockets.push((interface, socket));
                }
                Err(error) => {
                    log::debug!(
                        "Failed to send mDNS query on interface {}: {}",
                        interface,
                        error
                    );
                }
            }
        }

        if sockets.is_empty() {
            return Err(crate::Error::Debug(
                "failed to send mDNS query on any interface".to_string(),
            ));
        }

        let started = Instant::now();
        let mut devices: HashMap<String, WifiDiscoveredDevice> = HashMap::new();
        let (packet_tx, mut packet_rx) = mpsc::channel::<Vec<u8>>(sockets.len() * 4);
        let mut receive_tasks = Vec::new();

        for (interface, socket) in sockets {
            let packet_tx = packet_tx.clone();
            receive_tasks.push(tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                while let Some(remaining) = timeout.checked_sub(started.elapsed()) {
                    match tokio::time::timeout(remaining, socket.recv_from(&mut buffer)).await {
                        Ok(Ok((len, _))) => {
                            if packet_tx.send(buffer[..len].to_vec()).await.is_err() {
                                break;
                            }
                        }
                        Ok(Err(error)) => {
                            log::debug!(
                                "Failed to receive mDNS response on interface {}: {}",
                                interface,
                                error
                            );
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }));
        }
        drop(packet_tx);

        while let Some(remaining) = timeout.checked_sub(started.elapsed()) {
            match tokio::time::timeout(remaining, packet_rx.recv()).await {
                Ok(Some(packet)) => {
                    if let Err(error) = collect_mdns_devices(&packet, &mut devices) {
                        log::trace!("Ignoring malformed mDNS response: {:?}", error);
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        for task in receive_tasks {
            task.abort();
            let _ = task.await;
        }

        let devices: Vec<_> = devices.into_values().collect();
        log::debug!(
            "mDNS discovery found {} Enody service candidate(s)",
            devices.len()
        );
        Ok(devices)
    }

    fn create_mdns_socket(interface: Ipv4Addr) -> Result<UdpSocket, crate::Error> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        socket
            .set_reuse_address(true)
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        socket
            .bind(&SockAddr::from(SocketAddrV4::new(interface, 0)))
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        if !interface.is_unspecified() {
            socket
                .set_multicast_if_v4(&interface)
                .map_err(|error| crate::Error::Debug(error.to_string()))?;
        }
        // mDNS requires packets to be sent with an IP TTL of 255.
        socket
            .set_multicast_ttl_v4(255)
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        socket
            .set_nonblocking(true)
            .map_err(|error| crate::Error::Debug(error.to_string()))?;

        let socket: std::net::UdpSocket = socket.into();
        UdpSocket::from_std(socket).map_err(|error| crate::Error::Debug(error.to_string()))
    }

    fn mdns_outbound_interfaces() -> Vec<Ipv4Addr> {
        let mut interfaces = Vec::new();

        #[cfg(windows)]
        {
            let mut windows_interfaces = windows_mdns_outbound_interfaces();
            interfaces.append(&mut windows_interfaces);
        }

        if interfaces.is_empty() {
            interfaces.push(Ipv4Addr::UNSPECIFIED);
        }
        interfaces
    }

    #[cfg(windows)]
    /// Query and configure network interfaces on Windows computers
    /// for mDNS. This method was fully implemented by an LLM as I do
    /// not know the internals of Windows networking and do not care to learn.
    fn windows_mdns_outbound_interfaces() -> Vec<Ipv4Addr> {
        use windows_sys::Win32::{
            Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS},
            NetworkManagement::{
                IpHelper::{
                    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
                    GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL,
                    IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_IPV4_ENABLED, IP_ADAPTER_NO_MULTICAST,
                },
                Ndis::IfOperStatusUp,
            },
            Networking::WinSock::{AF_INET, SOCKADDR_IN},
        };

        let mut size = 15 * 1024u32;
        let mut buffer = vec![0u8; size as usize];
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;

        let mut result = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                flags,
                std::ptr::null(),
                buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut size,
            )
        };

        if result == ERROR_BUFFER_OVERFLOW {
            buffer.resize(size as usize, 0);
            result = unsafe {
                GetAdaptersAddresses(
                    AF_INET as u32,
                    flags,
                    std::ptr::null(),
                    buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
        }

        if result != ERROR_SUCCESS {
            log::debug!("GetAdaptersAddresses failed with Windows error {}", result);
            return Vec::new();
        }

        let mut interfaces = Vec::new();
        let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !adapter.is_null() {
            let adapter_ref = unsafe { &*adapter };
            let adapter_flags = unsafe { adapter_ref.Anonymous2.Flags };
            let multicast_capable = adapter_flags & IP_ADAPTER_NO_MULTICAST == 0;
            let ipv4_enabled = adapter_flags & IP_ADAPTER_IPV4_ENABLED != 0;
            let usable_adapter = adapter_ref.OperStatus == IfOperStatusUp
                && adapter_ref.IfType != IF_TYPE_SOFTWARE_LOOPBACK
                && adapter_ref.IfType != IF_TYPE_TUNNEL
                && multicast_capable
                && ipv4_enabled;

            if usable_adapter {
                let mut unicast = adapter_ref.FirstUnicastAddress;
                while !unicast.is_null() {
                    let socket_address = unsafe { (*unicast).Address };
                    if !socket_address.lpSockaddr.is_null()
                        && socket_address.iSockaddrLength as usize
                            >= std::mem::size_of::<SOCKADDR_IN>()
                    {
                        let sockaddr =
                            unsafe { &*(socket_address.lpSockaddr.cast::<SOCKADDR_IN>()) };
                        if sockaddr.sin_family == AF_INET {
                            let address = Ipv4Addr::from(unsafe {
                                sockaddr.sin_addr.S_un.S_addr.to_ne_bytes()
                            });
                            if !address.is_loopback()
                                && !address.is_link_local()
                                && !interfaces.contains(&address)
                            {
                                interfaces.push(address);
                            }
                        }
                    }
                    unicast = unsafe { (*unicast).Next };
                }
            }

            adapter = adapter_ref.Next;
        }

        log::debug!(
            "Windows mDNS outbound interfaces: {}",
            interfaces
                .iter()
                .map(Ipv4Addr::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        interfaces
    }

    fn build_mdns_ptr_query(query_id: u16) -> Result<Vec<u8>, crate::Error> {
        let mut packet = [0u8; 64];
        let len = WifiServiceQuestion
            .query(query_id, &mut packet)
            .map_err(|_| crate::Error::Serialization)?;
        Ok(packet[..len].to_vec())
    }

    struct WifiServiceQuestion;

    impl HostQuestions for WifiServiceQuestion {
        fn visit<F, E>(&self, mut f: F) -> Result<(), E>
        where
            F: FnMut(edge_mdns::HostQuestion) -> Result<(), E>,
            E: From<MdnsError>,
        {
            f(Question::new(
                NameSlice::new(WIFI_SERVICE_LABELS),
                Rtype::PTR,
                Class::from_int(0x8001),
            ))
        }
    }

    fn collect_mdns_devices(
        packet: &[u8],
        devices: &mut HashMap<String, WifiDiscoveredDevice>,
    ) -> Result<(), crate::Error> {
        let message =
            MdnsMessage::from_octets(packet).map_err(|_| crate::Error::InsufficientData)?;
        let mut records = MdnsRecords::default();

        for record in message
            .answer()
            .map_err(|_| crate::Error::Serialization)?
            .into_records::<AllRecordData<_, _>>()
        {
            collect_mdns_record(
                record.map_err(|_| crate::Error::Serialization)?,
                &mut records,
            );
        }

        for record in message
            .authority()
            .map_err(|_| crate::Error::Serialization)?
            .into_records::<AllRecordData<_, _>>()
        {
            collect_mdns_record(
                record.map_err(|_| crate::Error::Serialization)?,
                &mut records,
            );
        }

        for record in message
            .additional()
            .map_err(|_| crate::Error::Serialization)?
            .into_records::<AllRecordData<_, _>>()
        {
            collect_mdns_record(
                record.map_err(|_| crate::Error::Serialization)?,
                &mut records,
            );
        }

        let mut instance_names = records.ptr_instances;
        for name in records.srv_records.keys().chain(records.txt_records.keys()) {
            if name
                .trim_end_matches('.')
                .to_ascii_lowercase()
                .ends_with(WIFI_SERVICE_SUFFIX)
                && !instance_names
                    .iter()
                    .any(|existing| dns_name_eq(existing, name))
            {
                instance_names.push(name.clone());
            }
        }

        for instance in instance_names {
            let entry = devices
                .entry(instance.clone())
                .or_insert_with(|| WifiDiscoveredDevice {
                    instance: instance.clone(),
                    ..WifiDiscoveredDevice::default()
                });
            if let Some((host, port)) = records.srv_records.get(&instance) {
                entry.host = host.trim_end_matches('.').to_string();
                entry.port = Some(*port);
                entry.address = records.a_records.get(host).copied();
            }
            if let Some(txt) = records.txt_records.get(&instance) {
                entry.model = txt.get("model").cloned();
                entry.api_version = txt.get("api").and_then(|api| api.parse().ok());
                entry.host_id = txt.get("id").and_then(|id| id.parse().ok());
                entry.firmware_version = txt.get("firmware").or_else(|| txt.get("fw")).cloned();
                entry.http_port = txt.get("http").and_then(|port| port.parse().ok());
                entry.port = txt
                    .get("port")
                    .and_then(|port| port.parse().ok())
                    .or(entry.port);
                entry.protocol = txt.get("proto").cloned();
                entry.auth = txt.get("auth").cloned();
            }
        }

        for device in devices.values_mut() {
            if let Some(address) = records
                .a_records
                .get(&format!("{}.", device.host.trim_end_matches('.')))
            {
                device.address = Some(*address);
            }
        }

        Ok(())
    }

    fn collect_mdns_record(record: PeerAnswer<'_>, records: &mut MdnsRecords) {
        let name = dns_name_to_string(record.owner());

        match record.data() {
            AllRecordData::A(address) => {
                let [a, b, c, d] = address.addr().octets();
                records.a_records.insert(name, Ipv4Addr::new(a, b, c, d));
            }
            AllRecordData::Ptr(ptr)
                if record.owner().name_eq(&NameSlice::new(WIFI_SERVICE_LABELS)) =>
            {
                records
                    .ptr_instances
                    .push(dns_name_to_string(ptr.ptrdname()));
            }
            AllRecordData::Srv(srv) => {
                records
                    .srv_records
                    .insert(name, (dns_name_to_string(srv.target()), srv.port()));
            }
            AllRecordData::Txt(txt) => {
                records
                    .txt_records
                    .insert(name, parse_txt_record(txt.iter()));
            }
            _ => {}
        }
    }

    fn dns_name_to_string(name: &impl fmt::Display) -> String {
        let mut name = name.to_string();
        if !name.ends_with('.') {
            name.push('.');
        }
        name
    }

    #[derive(Default)]
    struct MdnsRecords {
        ptr_instances: Vec<String>,
        srv_records: HashMap<String, (String, u16)>,
        txt_records: HashMap<String, HashMap<String, String>>,
        a_records: HashMap<String, Ipv4Addr>,
    }

    fn parse_txt_record<'a>(items: impl IntoIterator<Item = &'a [u8]>) -> HashMap<String, String> {
        let mut values = HashMap::new();
        for item in items {
            if let Ok(item) = std::str::from_utf8(item) {
                if let Some((key, value)) = item.split_once('=') {
                    values.insert(key.to_string(), value.to_string());
                }
            }
        }
        values
    }

    fn dns_name_eq(left: &str, right: &str) -> bool {
        left.trim_end_matches('.')
            .eq_ignore_ascii_case(right.trim_end_matches('.'))
    }

    #[cfg(all(test, feature = "remote"))]
    mod tests {
        use super::*;
        use crate::message::{Command, CommandMessage, HostCommand};
        use edge_mdns::{
            domain::base::Ttl,
            host::{Host, Service, ServiceAnswers},
            HostAnswersMdnsHandler, MdnsHandler, MdnsRequest, MdnsResponse,
        };

        fn build_noise_responder(
            key: &[u8; 32],
            key_id: &str,
            host_id: Identifier,
        ) -> snow::HandshakeState {
            let params: snow::params::NoiseParams = WIFI_NOISE.parse().unwrap();
            let prologue = noise_prologue(key_id, host_id).unwrap();
            snow::Builder::new(params)
                .psk(0, key)
                .unwrap()
                .prologue(&prologue)
                .unwrap()
                .build_responder()
                .unwrap()
        }

        #[test]
        fn noise_handshake_encrypts_messages() {
            let key = [0x42u8; 32];
            let key_id = "ha-test";
            let host_id = uuid::Uuid::from_bytes([0x11u8; 16]);
            let mut initiator = build_noise_initiator(&key, key_id, host_id).unwrap();
            let mut responder = build_noise_responder(&key, key_id, host_id);

            let mut handshake_1 = [0u8; WIFI_FRAME_PAYLOAD_MAX_LEN];
            let handshake_1_len = initiator.write_message(&[], &mut handshake_1).unwrap();
            let mut empty = [0u8; 0];
            assert_eq!(
                responder
                    .read_message(&handshake_1[..handshake_1_len], &mut empty)
                    .unwrap(),
                0
            );

            let mut handshake_2 = [0u8; WIFI_FRAME_PAYLOAD_MAX_LEN];
            let handshake_2_len = responder.write_message(&[], &mut handshake_2).unwrap();
            assert_eq!(
                initiator
                    .read_message(&handshake_2[..handshake_2_len], &mut empty)
                    .unwrap(),
                0
            );

            let mut initiator = initiator.into_transport_mode().unwrap();
            let mut responder = responder.into_transport_mode().unwrap();
            let message =
                Message::Command(CommandMessage::root(Command::Host(HostCommand::Info), None));
            let plaintext = postcard::to_allocvec(&message).unwrap();

            let mut encrypted = [0u8; WIFI_FRAME_PAYLOAD_MAX_LEN];
            let encrypted_len = initiator.write_message(&plaintext, &mut encrypted).unwrap();
            assert!(encrypted_len > plaintext.len());

            let mut decrypted = [0u8; WIFI_MESSAGE_MAX_LEN];
            let decrypted_len = responder
                .read_message(&encrypted[..encrypted_len], &mut decrypted)
                .unwrap();
            assert_eq!(&decrypted[..decrypted_len], plaintext.as_slice());
        }

        #[test]
        fn mdns_discovery_parses_edge_mdns_service_answers() {
            let host_id = uuid::Uuid::from_bytes([0x22u8; 16]);
            let host_id_string = host_id.to_string();
            let txt = [
                ("model", "ep01"),
                ("api", "1"),
                ("id", host_id_string.as_str()),
                ("fw", "1.2.3"),
                ("http", "80"),
                ("port", "8788"),
                ("proto", WIFI_PROTOCOL),
                ("auth", WIFI_AUTH),
            ];
            let host = Host {
                hostname: "enody-test",
                ipv4: Ipv4Addr::new(192, 168, 1, 45),
                ipv6: std::net::Ipv6Addr::UNSPECIFIED,
                ttl: Ttl::from_secs(60),
            };
            let service = Service {
                name: "enody-test",
                priority: 0,
                weight: 0,
                service: "_enody",
                protocol: "_tcp",
                port: WIFI_PORT,
                service_subtypes: &[],
                txt_kvs: &txt,
            };
            let answers = ServiceAnswers::new(&host, &service);
            let mut handler = HostAnswersMdnsHandler::new(&answers);
            let mut response = [0u8; 1024];

            let MdnsResponse::Reply { data, .. } = handler
                .handle(MdnsRequest::None, &mut response)
                .expect("edge-mdns should build a service response")
            else {
                panic!("edge-mdns service response should contain answers");
            };

            let mut devices = HashMap::new();
            collect_mdns_devices(data, &mut devices).unwrap();

            let device = devices
                .get("enody-test._enody._tcp.local.")
                .expect("service instance should be discovered");
            assert_eq!(device.instance, "enody-test._enody._tcp.local.");
            assert_eq!(device.host, "enody-test.local");
            assert_eq!(device.address, Some(Ipv4Addr::new(192, 168, 1, 45)));
            assert_eq!(device.model.as_deref(), Some("ep01"));
            assert_eq!(device.api_version, Some(1));
            assert_eq!(device.host_id, Some(host_id));
            assert_eq!(device.firmware_version.as_deref(), Some("1.2.3"));
            assert_eq!(device.http_port, Some(80));
            assert_eq!(device.port, Some(WIFI_PORT));
            assert_eq!(device.protocol.as_deref(), Some(WIFI_PROTOCOL));
            assert_eq!(device.auth.as_deref(), Some(WIFI_AUTH));
        }
    }
}
