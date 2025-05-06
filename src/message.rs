use serde::{
	Deserialize,
	Serialize
};
use tokio::task::Id;

use super::{
    Configuration,
    Identifier
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Message<InternalCommand, InternalEvent> {
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command<InternalCommand> {
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
pub enum Event<InternalEvent> {
	Error(ErrorEvent),
	Internal(InternalEvent),
	Host(HostEvent),
	Runtime(RuntimeEvent),
	Environment(EnvironmentEvent),
	Fixture(FixtureEvent),
	Source(SourceEvent),
	Emitter(EmitterEvent)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Handle {
	pub identifier: Option<Identifier>
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PowerInfo {
	
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum PowerCommand {
	Info(PowerInfo),
	Limit,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ErrorEvent {
	Unknown
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostCommand {
	Info,
	Reboot
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostEvent {
	Info(HostInfo),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostInfo {
	pub identifier: Identifier
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeCommand {
	Info,
	EnvironmentList(Handle),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RuntimeEvent {
	Info(RuntimeInfo),
    EnvironmentEnter(EnvironmentInfo),
    EnvironmentExit(EnvironmentInfo),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeInfo {

}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentCommand {
	Info,
	FixtureList(Handle),
	Display(Configuration),
	Power(PowerCommand)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EnvironmentEvent {
    Info(EnvironmentInfo),
    FixtureEnter(FixtureInfo),
    FixtureExit(FixtureInfo)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvironmentInfo {

}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureCommand {
	Info,
	SourceList(Handle),
	Display(Configuration),
	Power(PowerCommand),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FixtureEvent {
	Info(FixtureInfo),
	SourceList(Handle, SourceInfo),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FixtureInfo {

}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceCommand {
    Info,
    EmitterList(Handle),
    Display(Configuration, Flux),
    Power(PowerCommand)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SourceEvent {
	Info(SourceInfo),
	Display(Configuration, Flux)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceInfo {

}

pub type Measurement = f32;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Flux {
	Relative(Measurement),
	Absolute(Measurement)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterCommand {
	Info,
	FluxRange,
	FluxSet(Flux),
	CharacteristicSpectralDistribution(Handle, u32)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum EmitterEvent {
	Info(EmitterInfo),
	FluxRange(u32, u32),
	FluxSet(Flux),
	CharacteristicSpectralDistribution(Handle, Vec<(f32, f32)>)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmitterInfo {
	
}
