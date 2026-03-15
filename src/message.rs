#![allow(clippy::large_enum_variant)]

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::{spectral::SpectralSample, Identifier, Measurement};

pub const SPECTRAL_SAMPLE_BATCH_SIZE: usize = 32;
const LOG_EVENT_BUFFER_SIZE: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Flux {
    Relative(Measurement),
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Chromaticity {
    pub x: Measurement,
    pub y: Measurement,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Configuration {
    Flux,
    Blackbody(Measurement),
    Chromatic(Chromaticity),
    Spectral,
    Manual,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct Version {
    major: u8,
    minor: u8,
    patch: u16,
}

impl Version {
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Message {
    Command(CommandMessage),
    Event(EventMessage),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandMessage {
    pub identifier: Identifier,
    pub context: Option<Identifier>,
    pub resource: Option<Identifier>,
    pub command: Command,
}

impl CommandMessage {
    pub fn root(command: Command, resource: Option<Identifier>) -> Self {
        Self {
            identifier: uuid::Uuid::new_v4(),
            context: None,
            resource,
            command,
        }
    }

    pub fn child(&self, command: Command, resource: Option<Identifier>) -> Self {
        Self {
            identifier: uuid::Uuid::new_v4(),
            context: Some(self.identifier),
            resource,
            command,
        }
    }

    pub fn identifier(&self) -> &Identifier {
        &self.identifier
    }

    pub fn context(&self) -> Option<Identifier> {
        self.context
    }

    pub fn resource(&self) -> Option<Identifier> {
        self.resource
    }

    pub fn action(&self) -> &Command {
        &self.command
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    Internal,
    Host(HostCommand),
    Runtime(RuntimeCommand),
    Environment(EnvironmentCommand),
    Fixture(FixtureCommand),
    Source(SourceCommand),
    Emitter(EmitterCommand),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventMessage {
    pub identifier: Identifier,
    pub context: Option<Identifier>,
    pub resource: Option<Identifier>,
    pub event: Event,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Event {
    Error(crate::Error),
    Internal,
    Host(HostEvent),
    Runtime(RuntimeEvent),
    Environment(EnvironmentEvent),
    Fixture(FixtureEvent),
    Source(SourceEvent),
    Emitter(EmitterEvent),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostCommand {
    Info,
    FixtureCount,
    FixtureInfo(u32),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostEvent {
    Info(HostInfo),
    FixtureCount(u32),
    FixtureInfo(FixtureInfo),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HostInfo {
    pub version: Version,
    pub identifier: Identifier,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureCommand {
    Info,
    Display(Configuration, Flux),
    SourceCount,
    SourceInfo(u32),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureEvent {
    Info(FixtureInfo),
    Display(Configuration, Flux),
    SourceCount(u32),
    SourceInfo(SourceInfo),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FixtureInfo {
    pub identifier: Identifier,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceCommand {
    Info,
    Display(Configuration, Flux),
    EmitterCount,
    EmitterInfo(u32),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceEvent {
    Info(SourceInfo),
    Display(Configuration, Flux),
    EmitterCount(u32),
    EmitterInfo(EmitterInfo),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceInfo {
    pub identifier: Identifier,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterCommand {
    Info,
    FluxRange,
    FluxSet(Flux),
    SpectralData(SpectralDataCommand),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterEvent {
    Info(EmitterInfo),
    FluxRange(Flux, Flux),
    FluxSet(Flux),
    SpectralData(SpectralDataEvent),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmitterInfo {
    identifier: Identifier,
}

impl EmitterInfo {
    pub fn new(identifier: Identifier) -> Self {
        Self { identifier }
    }
}

impl EmitterInfo {
    pub fn identifier(&self) -> Identifier {
        self.identifier
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SpectralDataCommand {
    Info,
    Domain,
    SampleCount,
    Sample(u32),
    SampleBatch(u32, u32),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SpectralDataEvent {
    Info(SpectralDataInfo),
    Domain(Measurement, Measurement),
    SampleCount(u32),
    Sample(SpectralSample),
    SampleBatch(Vec<SpectralSample, SPECTRAL_SAMPLE_BATCH_SIZE>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpectralDataInfo {
    identifier: Identifier,
}

impl SpectralDataInfo {
    pub fn new(identifier: Identifier) -> Self {
        Self { identifier }
    }

    pub fn identifier(&self) -> Identifier {
        self.identifier
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeCommand {
    Info,
    Host,
    EnvironmentCount,
    EnvironmentInfo(u32),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeEvent {
    Info(RuntimeInfo),
    Log(LogEvent),
    Host(HostInfo),
    EnvironmentCount(u32),
    EnvironmentInfo(EnvironmentInfo),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeInfo {
    pub version: Version,
    pub identifier: Identifier,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogEvent {
    pub level: LogLevel,
    pub output: String<LOG_EVENT_BUFFER_SIZE>,
}

#[repr(u8)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum LogLevel {
    Error = 1,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentCommand {
    Info,
    Display(Configuration, Flux),
    RuntimeCount,
    RuntimeInfo(u32),
    FixtureCount,
    FixtureInfo(u32),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentEvent {
    Info(EnvironmentInfo),
    Display(Configuration, Flux),
    RuntimeCount(u32),
    RuntimeInfo(RuntimeInfo),
    FixtureCount(u32),
    FixtureInfo(FixtureInfo, u32),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvironmentInfo {
    pub identifier: Identifier,
}
