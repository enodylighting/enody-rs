//! Shared command/event protocol and topology wire types.
//!
//! All remote transports exchange [`crate::message::Message`] values. Commands carry a command
//! UUID in [`crate::message::CommandMessage::identifier`], and response events use that UUID as
//! their [`crate::message::EventMessage::context`] so `runtime::remote::RemoteRuntime`
//! can match replies to pending commands.

#![allow(clippy::large_enum_variant)]

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::{spectral::SpectralSample, Identifier, Measurement};

/// Maximum number of spectral samples returned by one sample-batch response.
pub const SPECTRAL_SAMPLE_BATCH_SIZE: usize = 32;
const LOG_EVENT_BUFFER_SIZE: usize = 256;

/// Target luminous flux.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Flux {
    /// Relative flux, usually normalized between `0.0` and `1.0`.
    Relative(Measurement),
}

/// CIE 1931 xy chromaticity coordinate.
#[derive(Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Chromaticity {
    /// CIE x coordinate.
    pub x: Measurement,
    /// CIE y coordinate.
    pub y: Measurement,
}

/// Requested output configuration for a fixture or source.
#[derive(Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Configuration {
    /// Flux-only command without a new color target.
    Flux,
    /// Blackbody color temperature in kelvin.
    Blackbody(Measurement),
    /// Chromaticity target.
    Chromatic(Chromaticity),
    /// Spectral target mode.
    Spectral,
    /// Manual emitter mix mode.
    Manual,
}

/// Semantic firmware or protocol version.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct Version {
    major: u8,
    minor: u8,
    patch: u16,
}

impl Version {
    /// Creates a version from major, minor, and patch components.
    pub fn new(major: u8, minor: u8, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl core::fmt::Display for Version {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl core::str::FromStr for Version {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('.');
        let major = parts
            .next()
            .ok_or("missing major")?
            .parse::<u8>()
            .map_err(|_| "invalid major")?;
        let minor = parts
            .next()
            .ok_or("missing minor")?
            .parse::<u8>()
            .map_err(|_| "invalid minor")?;
        let patch = parts
            .next()
            .ok_or("missing patch")?
            .parse::<u16>()
            .map_err(|_| "invalid patch")?;
        if parts.next().is_some() {
            return Err("too many version parts");
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

/// Top-level wire message sent over a transport.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Message {
    /// Command sent to a runtime or resource.
    Command(CommandMessage),
    /// Event sent by a runtime or resource.
    Event(EventMessage),
}

/// Command envelope with routing and response-correlation metadata.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandMessage {
    /// Unique command identifier.
    pub identifier: Identifier,
    /// Optional parent command identifier.
    pub context: Option<Identifier>,
    /// Optional resource identifier targeted by this command.
    pub resource: Option<Identifier>,
    /// Command payload.
    pub command: Command,
}

impl CommandMessage {
    /// Creates a root command with a new identifier and no parent context.
    pub fn root(command: Command, resource: Option<Identifier>) -> Self {
        Self {
            identifier: uuid::Uuid::new_v4(),
            context: None,
            resource,
            command,
        }
    }

    /// Creates a child command whose context is this command's identifier.
    pub fn child(&self, command: Command, resource: Option<Identifier>) -> Self {
        Self {
            identifier: uuid::Uuid::new_v4(),
            context: Some(self.identifier),
            resource,
            command,
        }
    }

    /// Returns this command's unique identifier.
    pub fn identifier(&self) -> &Identifier {
        &self.identifier
    }

    /// Returns the parent command context, if any.
    pub fn context(&self) -> Option<Identifier> {
        self.context
    }

    /// Returns the targeted resource identifier, if any.
    pub fn resource(&self) -> Option<Identifier> {
        self.resource
    }

    /// Returns the command payload.
    pub fn action(&self) -> &Command {
        &self.command
    }
}

/// Command payload grouped by protocol resource.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    /// Internal command reserved for implementation use.
    Internal,
    /// Host-level command.
    Host(HostCommand),
    /// Runtime-level command.
    Runtime(RuntimeCommand),
    /// Environment-level command.
    Environment(EnvironmentCommand),
    /// Fixture-level command.
    Fixture(FixtureCommand),
    /// Source-level command.
    Source(SourceCommand),
    /// Emitter-level command.
    Emitter(EmitterCommand),
}

/// Event envelope with routing and command-correlation metadata.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventMessage {
    /// Unique event identifier.
    pub identifier: Identifier,
    /// Command identifier this event responds to, if any.
    pub context: Option<Identifier>,
    /// Resource identifier that emitted the event, if any.
    pub resource: Option<Identifier>,
    /// Event payload.
    pub event: Event,
}

/// Event payload grouped by protocol resource.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Event {
    /// Error response.
    Error(crate::Error),
    /// Internal event reserved for implementation use.
    Internal,
    /// Host-level event.
    Host(HostEvent),
    /// Runtime-level event.
    Runtime(RuntimeEvent),
    /// Environment-level event.
    Environment(EnvironmentEvent),
    /// Fixture-level event.
    Fixture(FixtureEvent),
    /// Source-level event.
    Source(SourceEvent),
    /// Emitter-level event.
    Emitter(EmitterEvent),
}

/// Maximum number of network filters in a scan command.
pub const NETWORK_SCAN_FILTER_MAX_LEN: usize = 4;
/// Maximum number of network results in a scan-complete event.
pub const NETWORK_SCAN_RESULT_MAX_LEN: usize = 16;

/// Commands accepted by a host.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostCommand {
    /// Request host metadata.
    Info,
    /// Request the number of fixtures.
    FixtureCount,
    /// Request metadata for the fixture at the given index.
    FixtureInfo(u32),
    /// Scan networks matching the supplied filters.
    NetworkScan(Vec<Network, NETWORK_SCAN_FILTER_MAX_LEN>),
    /// Join a network with credentials.
    NetworkJoin(Network, NetworkCredentials),
}

/// Events emitted by a host.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostEvent {
    /// Host metadata response.
    Info(HostInfo),
    /// Fixture count response.
    FixtureCount(u32),
    /// Fixture metadata response.
    FixtureInfo(FixtureInfo),
    /// Network scan started.
    NetworkScanStart(Vec<Network, NETWORK_SCAN_FILTER_MAX_LEN>),
    /// Network scan completed.
    NetworkScanComplete(Vec<Network, NETWORK_SCAN_RESULT_MAX_LEN>),
    /// Network join started.
    NetworkJoinStart(Network),
    /// Network join completed.
    NetworkJoinComplete(Network),
}

/// Metadata describing a product host.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HostInfo {
    /// Firmware version reported by the host.
    pub version: Version,
    /// Stable host identifier.
    pub identifier: Identifier,
}

/// Network descriptor used for scanning and joining.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Network {
    /// WiFi network.
    Wifi(WifiNetwork),
}

/// Credentials for joining a network.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum NetworkCredentials {
    /// No credentials, used for open networks.
    None,
    /// WiFi credentials.
    Wifi(WifiCredentials),
}

/// Maximum SSID length accepted by the wire protocol.
pub const WIFI_SSID_MAX_LEN: usize = 32;
/// Maximum WiFi password length accepted by the wire protocol.
pub const WIFI_PASSWORD_MAX_LEN: usize = 64;

/// WiFi authentication class reported by a scan result.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum WifiAuth {
    /// Authentication type was not reported.
    Unknown,
    /// Open network.
    Open,
    /// Secured network.
    Secured,
}

/// WiFi network scan filter or scan result.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct WifiNetwork {
    /// Service set identifier.
    pub ssid: Option<String<WIFI_SSID_MAX_LEN>>,
    /// Basic service set identifier.
    pub bssid: Option<[u8; 6]>,
    /// WiFi channel.
    pub channel: Option<u8>,
    /// Signal strength in dBm.
    pub rssi: Option<i8>,
    /// Authentication class.
    pub auth: Option<WifiAuth>,
}

/// WiFi-specific credentials.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum WifiCredentials {
    /// WPA/WPA2-style password.
    Password(String<WIFI_PASSWORD_MAX_LEN>),
}

/// Commands accepted by a fixture.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureCommand {
    /// Request fixture metadata.
    Info,
    /// Display a configuration at a target flux.
    Display(Configuration, Flux),
    /// Request the number of sources.
    SourceCount,
    /// Request metadata for the source at the given index.
    SourceInfo(u32),
}

/// Events emitted by a fixture.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureEvent {
    /// Fixture metadata response.
    Info(FixtureInfo),
    /// Display command response.
    Display(Configuration, Flux),
    /// Source count response.
    SourceCount(u32),
    /// Source metadata response.
    SourceInfo(SourceInfo),
}

/// Metadata describing a fixture.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FixtureInfo {
    /// Stable fixture identifier.
    pub identifier: Identifier,
}

/// Commands accepted by a source.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceCommand {
    /// Request source metadata.
    Info,
    /// Display a configuration at a target flux.
    Display(Configuration, Flux),
    /// Request the number of emitters.
    EmitterCount,
    /// Request metadata for the emitter at the given index.
    EmitterInfo(u32),
}

/// Events emitted by a source.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceEvent {
    /// Source metadata response.
    Info(SourceInfo),
    /// Display command response.
    Display(Configuration, Flux),
    /// Emitter count response.
    EmitterCount(u32),
    /// Emitter metadata response.
    EmitterInfo(EmitterInfo),
}

/// Metadata describing a source.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceInfo {
    /// Stable source identifier.
    pub identifier: Identifier,
}

/// Commands accepted by an emitter.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterCommand {
    /// Request emitter metadata.
    Info,
    /// Request the supported flux range.
    FluxRange,
    /// Set the target flux.
    FluxSet(Flux),
    /// Request spectral data.
    SpectralData(SpectralDataCommand),
}

/// Events emitted by an emitter.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterEvent {
    /// Emitter metadata response.
    Info(EmitterInfo),
    /// Flux range response as `(minimum, maximum)`.
    FluxRange(Flux, Flux),
    /// Flux set response.
    FluxSet(Flux),
    /// Spectral data response.
    SpectralData(SpectralDataEvent),
}

/// Metadata describing an emitter.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmitterInfo {
    identifier: Identifier,
}

impl EmitterInfo {
    /// Creates emitter metadata for an identifier.
    pub fn new(identifier: Identifier) -> Self {
        Self { identifier }
    }
}

impl EmitterInfo {
    /// Returns the emitter identifier.
    pub fn identifier(&self) -> Identifier {
        self.identifier
    }
}

/// Commands accepted by an emitter's spectral data endpoint.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SpectralDataCommand {
    /// Request spectral data metadata.
    Info,
    /// Request the wavelength domain.
    Domain,
    /// Request the number of samples.
    SampleCount,
    /// Request one sample by index.
    Sample(u32),
    /// Request samples in the half-open index range `[start, end)`.
    SampleBatch(u32, u32),
}

/// Events emitted by an emitter's spectral data endpoint.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SpectralDataEvent {
    /// Spectral data metadata response.
    Info(SpectralDataInfo),
    /// Wavelength domain response as `(minimum, maximum)`.
    Domain(Measurement, Measurement),
    /// Number of samples available.
    SampleCount(u32),
    /// One spectral sample.
    Sample(SpectralSample),
    /// Batch of spectral samples.
    SampleBatch(Vec<SpectralSample, SPECTRAL_SAMPLE_BATCH_SIZE>),
}

/// Metadata describing a spectral data resource.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpectralDataInfo {
    identifier: Identifier,
}

impl SpectralDataInfo {
    /// Creates spectral data metadata for an identifier.
    pub fn new(identifier: Identifier) -> Self {
        Self { identifier }
    }

    /// Returns the spectral data identifier.
    pub fn identifier(&self) -> Identifier {
        self.identifier
    }
}

/// Commands accepted by a runtime.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeCommand {
    /// Request runtime metadata.
    Info,
    /// Request the host metadata.
    Host,
    /// Request the number of environments.
    EnvironmentCount,
    /// Request metadata for the environment at the given index.
    EnvironmentInfo(u32),
    /// Read a stored setting.
    SettingGet(SettingKey),
    /// Write a stored setting.
    SettingSet(SettingKey, SettingValue),
    /// Delete a stored setting.
    SettingDelete(SettingKey),
    /// Reset settings to device defaults.
    SettingReset,
    /// Generate a WiFi authentication token.
    TokenGenerate,
    /// Revoke a WiFi authentication token.
    TokenRevoke(TokenKeyId),
}

/// Events emitted by a runtime.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeEvent {
    /// Runtime metadata response.
    Info(RuntimeInfo),
    /// Runtime log output.
    Log(LogEvent),
    /// Host metadata response.
    Host(HostInfo),
    /// Environment count response.
    EnvironmentCount(u32),
    /// Environment metadata response.
    EnvironmentInfo(EnvironmentInfo),
    /// Stored setting response.
    SettingGet(SettingKey, StoredSetting),
    /// Stored setting write response.
    SettingSet(SettingKey),
    /// Stored setting delete response.
    SettingDelete(SettingKey),
    /// Settings reset response.
    SettingReset,
    /// Token generation has started.
    TokenGenerateStart,
    /// Token generation requires the given physical approval instruction.
    TokenGenerateApproval(TokenApprovalMethod),
    /// Token generation completed.
    TokenGenerated(Token),
    /// Token revocation completed.
    TokenRevoked(TokenKeyId),
}

/// Metadata describing a runtime.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeInfo {
    /// Runtime firmware or protocol version.
    pub version: Version,
    /// Stable runtime identifier.
    pub identifier: Identifier,
}

/// Log output emitted by a remote runtime.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogEvent {
    /// Log severity.
    pub level: LogLevel,
    /// Log message text.
    pub output: String<LOG_EVENT_BUFFER_SIZE>,
}

/// Runtime log severity.
#[repr(u8)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum LogLevel {
    /// Error-level log message.
    Error = 1,
    /// Warning-level log message.
    Warn,
    /// Informational log message.
    Info,
    /// Debug log message.
    Debug,
    /// Trace log message.
    Trace,
}

/// Maximum stored setting key length.
pub const SETTING_KEY_MAX_LEN: usize = 64;
/// Stored setting key.
pub type SettingKey = String<SETTING_KEY_MAX_LEN>;

/// Maximum serialized stored setting value length.
pub const SETTING_VALUE_MAX_LEN: usize = 128;
/// Serialized stored setting value.
pub type SettingValue = Vec<u8, SETTING_VALUE_MAX_LEN>;

/// Stored setting visibility and value state.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum StoredSetting {
    /// The setting does not exist.
    Missing,
    /// The setting is public and carries serialized bytes.
    Public(SettingValue),
    /// The setting exists but cannot be read by this caller.
    Private,
}

/// Maximum token key identifier string length.
pub const TOKEN_STRING_MAX_LEN: usize = 64;
/// Maximum token key material length.
pub const TOKEN_DATA_MAX_LEN: usize = 32;

type TokenKeyId = String<TOKEN_STRING_MAX_LEN>;
type TokenApprovalMethod = String<TOKEN_STRING_MAX_LEN>;

/// WiFi authentication token.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Token {
    /// Host identifier this token authenticates to.
    pub host_id: crate::Identifier,
    /// Token key identifier sent in the WiFi hello frame.
    pub key_id: TokenKeyId,
    /// Shared key material.
    pub data: Vec<u8, TOKEN_DATA_MAX_LEN>,
}

/// Commands accepted by an environment.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentCommand {
    /// Request environment metadata.
    Info,
    /// Display a configuration across the environment.
    Display(Configuration, Flux),
    /// Request the number of runtimes.
    RuntimeCount,
    /// Request metadata for the runtime at the given index.
    RuntimeInfo(u32),
    /// Request the number of fixtures across the environment.
    FixtureCount,
    /// Request metadata for the fixture at the given index.
    FixtureInfo(u32),
}

/// Events emitted by an environment.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentEvent {
    /// Environment metadata response.
    Info(EnvironmentInfo),
    /// Environment display response.
    Display(Configuration, Flux),
    /// Runtime count response.
    RuntimeCount(u32),
    /// Runtime metadata response.
    RuntimeInfo(RuntimeInfo),
    /// Fixture count response.
    FixtureCount(u32),
    /// Fixture metadata response and its runtime index.
    FixtureInfo(FixtureInfo, u32),
}

/// Metadata describing an environment.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvironmentInfo {
    /// Stable environment identifier.
    pub identifier: Identifier,
}
