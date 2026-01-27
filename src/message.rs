use heapless::{ String, Vec };
use serde::{ Deserialize, Serialize };

use crate::{
	Identifier,
	Measurement,
	spectral::SpectralSample
};

const SPECTRAL_SAMPLE_BATCH_SIZE: usize = 32;
const LOG_EVENT_BUFFER_SIZE: usize = 128;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Flux {
	Relative(Measurement)
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Chromaticity {
    pub x: Measurement,
    pub y: Measurement
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Configuration {
    Flux,
	Blackbody(Measurement),
	Chromatic(Chromaticity),
    Spectral,
    Manual
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Version {
	major: u8,
	minor: u8,
	patch: u16
}

impl core::fmt::Display for Version {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
	}
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Message<InternalCommand = (), InternalEvent = ()> {
	Command(CommandMessage<InternalCommand>),
	Event(EventMessage<InternalEvent>)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandMessage<InternalCommand> {
	pub identifier: Identifier,
	pub context: Option<Identifier>,
	pub resource: Option<Identifier>,
	pub command: Command<InternalCommand>
}

impl<InternalCommand> CommandMessage<InternalCommand> {
    pub fn root(command: Command<InternalCommand>, resource: Option<Identifier>) -> Self {
        Self {
            identifier: uuid::Uuid::new_v4(),
            context: None,
            resource,
            command
        }
    }

    pub fn child(&self, command: Command<InternalCommand>, resource: Option<Identifier>) -> Self {
        Self {
            identifier: uuid::Uuid::new_v4(),
            context: Some(self.identifier.clone()),
            resource,
            command
        }
    }

    pub fn identifier(&self) -> &Identifier {
        &self.identifier
    }

    pub fn context(&self) -> Option<Identifier> {
        self.context.clone()
    }

    pub fn resource(&self) -> Option<Identifier> {
        self.resource.clone()
    }

    pub fn action(&self) -> &Command<InternalCommand> {
        &self.command
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command<InternalCommand = ()> {
	Internal(InternalCommand),
	Host(HostCommand),
	Runtime(RuntimeCommand),
	Environment(EnvironmentCommand),
	Fixture(FixtureCommand),
	Source(SourceCommand),
	Emitter(EmitterCommand)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventMessage<InternalEvent> {
	pub identifier: Identifier,
	pub context: Option<Identifier>,
	pub resource: Option<Identifier>,
	pub event: Event<InternalEvent>
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Event<InternalEvent = ()> {
	Error(crate::Error),
	Internal(InternalEvent),
	Host(HostEvent),
	Runtime(RuntimeEvent),
	Environment(EnvironmentEvent),
	Fixture(FixtureEvent),
	Source(SourceEvent),
	Emitter(EmitterEvent)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostCommand {
	Info,
	FixtureCount,
	FixtureInfo(u32)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostEvent {
	Info(HostInfo),
	FixtureCount(u32),
	FixtureInfo(FixtureInfo)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostInfo {
	pub version: Version,
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureCommand {
	Info,
	Display(Configuration, Flux),
	SourceCount,
	SourceInfo(u32)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureEvent {
	Info(FixtureInfo),
	Display(Configuration, Flux),
	SourceCount(u32),
	SourceInfo(SourceInfo)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FixtureInfo {
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceCommand {
    Info,
	Display(Configuration, Flux),
	EmitterCount,
    EmitterInfo(u32)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceEvent {
	Info(SourceInfo),
	Display(Configuration, Flux),
	EmitterCount(u32),
	EmitterInfo(EmitterInfo)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceInfo {
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterCommand {
	Info,
	FluxRange,
	FluxSet(Flux),
	SpectralData(Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterEvent {
	Info(EmitterInfo),
	FluxRange(Flux, Flux),
	FluxSet(Flux),
	SpectralData(SpectralDataInfo, Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmitterInfo {
	identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SpectralDataCommand {
	Info,
	SampleCount,
	Sample(u32),
	SampleBatch(u32, u32)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SpectralDataEvent {
	Info(SpectralDataInfo),
	SampleCount(u32),
	Sample(SpectralSample),
	SampleBatch(Vec<SpectralSample, SPECTRAL_SAMPLE_BATCH_SIZE>)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpectralDataInfo {
	identifier: Identifier,
	domain: (Measurement, Measurement)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeCommand {
	Info,
	Host,
	EnvironmentCount,
	EnvironmentInfo(u32)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeEvent {
	Info(RuntimeInfo),
	Log(LogEvent),
	Host(HostInfo),
	EnvironmentCount(u32),
	EnvironmentInfo(EnvironmentInfo)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeInfo {
	pub version: Version,
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogEvent {
    pub level: LogLevel,
    pub output: String<LOG_EVENT_BUFFER_SIZE>
}

#[repr(u8)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum LogLevel {
    Error = 1,
    Warn,
    Info,
    Debug,
    Trace
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentCommand {
	Info,
	Display(Configuration, Flux),
	RuntimeCount,
	RuntimeInfo(u32),
	FixtureCount,
	FixtureInfo(u32)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentEvent {
    Info(EnvironmentInfo),
	Display(Configuration, Flux),
	RuntimeCount(u32),
	RuntimeInfo(RuntimeInfo),
	FixtureCount(u32),
	FixtureInfo(FixtureInfo, u32)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvironmentInfo {
	pub identifier: Identifier
}