use serde::{
	Deserialize,
	Serialize
};

use super::Identifier;

pub type Measurement = f32;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum Flux {
	Relative(Measurement)
}

#[derive(Clone, Debug, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct Chromaticity {
    pub x: Measurement,
    pub y: Measurement
}

#[derive(Clone, Debug, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
pub enum Configuration {
    Flux,
	Blackbody(Measurement),
	Chromatic(Chromaticity),
    Spectral,
    Manual
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
pub struct Handle {
	pub identifier: Identifier
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
	Info
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostEvent {
	Info(HostInfo),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostInfo {
	pub version: Version,
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeCommand {
	Info,
	EnvironmentList(Option<Handle>)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeEvent {
	Info(RuntimeInfo),
	Log(LogEvent),
	EnvironmentList(Handle, Option<EnvironmentInfo>)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeInfo {
	pub version: Version,
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogEvent {
    pub level: LogLevel,
    pub output: String
}

#[repr(usize)]
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
	EnvironmentList(Handle, Option<EnvironmentInfo>),
	FixtureList(Option<Handle>),
	Display(Configuration, Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentEvent {
    Info(EnvironmentInfo),
	EnvironmentList(Handle, Option<EnvironmentInfo>),
	FixtureList(Handle, Option<FixtureInfo>),
	Display(Configuration, Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvironmentInfo {
	pub identifier: Identifier,
	pub fixture_count: usize
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureCommand {
	Info,
	SourceList(Option<Handle>),
	Display(Configuration, Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureEvent {
	Info(FixtureInfo),
	SourceList(Handle, Option<SourceInfo>),
	Display(Configuration, Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FixtureInfo {
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceCommand {
    Info,
    EmitterList(Option<Handle>),
    Display(Configuration, Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceEvent {
	Info(SourceInfo),
	EmitterList(Handle, Option<EmitterInfo>),
	Display(Configuration, Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceInfo {
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterCommand {
	Info,
	FluxRange,
	FluxSet(Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterEvent {
	Info(EmitterInfo),
	FluxRange(Flux, Flux),
	FluxSet(Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmitterInfo {
	identifier: Identifier
}
