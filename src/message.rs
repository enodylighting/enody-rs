use super::{
    Configuration,
    Identifier
};

pub enum Message<InternalCommand = (), InternalEvent = ()> {
	Command(CommandMessage<InternalCommand>),
	Event(EventMessage<InternalEvent>)
}

pub struct CommandMessage<InternalCommand = ()> {
	pub identifier: Identifier,
	pub context: Option<Identifier>,
	pub resource: Option<Identifier>,
	pub command: Command<InternalCommand>
}

pub struct EventMessage<InternalEvent = ()> {
	pub identifier: Identifier,
	pub context: Option<Identifier>,
	pub resource: Option<Identifier>,
	pub event: Event<InternalEvent>
}

pub enum Event<InternalEvent = ()> {
	Error(ErrorEvent),
	Internal(InternalEvent),
	Host(HostEvent),
	Runtime(RuntimeEvent),
	Environment(EnvironmentEvent),
	Fixture(FixtureEvent),
	Source(SourceEvent),
	Emitter(EmitterEvent)
}

pub struct Handle {
	pub identifier: Option<Identifier>
}

pub enum Command<InternalCommand = ()> {
	Internal(InternalCommand),
	Host(HostCommand),
	Runtime(RuntimeCommand),
	Environment(EnvironmentCommand),
	Fixture(FixtureCommand),
	Source(SourceCommand),
	Emitter(EmitterCommand)
}

pub struct PowerInfo {
	
}

pub enum PowerCommand {
	Info(PowerInfo),
	Limit,
}

pub enum ErrorEvent {
	Unknown
}

pub enum HostCommand {
	Info,
	Reboot
}

pub enum HostEvent {
	Info(HostInfo),
}

pub struct HostInfo {

}

pub enum RuntimeCommand {
	Info,
	EnvironmentList(Handle),
}

pub enum RuntimeEvent {
	Info(RuntimeInfo),
    EnvironmentEnter(EnvironmentInfo),
    EnvironmentExit(EnvironmentInfo),
}

pub struct RuntimeInfo {

}

pub enum EnvironmentCommand {
	Info,
	FixtureList(Handle),
	Display(Configuration),
	Power(PowerCommand)
}

pub enum EnvironmentEvent {
    Info(EnvironmentInfo),
    FixtureEnter(FixtureInfo),
    FixtureExit(FixtureInfo)
}

pub struct EnvironmentInfo {

}

pub enum FixtureCommand {
	Info,
	SourceList(Handle),
	Display(Configuration),
	Power(PowerCommand),
}

pub enum FixtureEvent {
	Info(FixtureInfo),
	SourceList(Handle, SourceInfo),
}

pub struct FixtureInfo {

}

pub enum SourceCommand {
    Info,
    EmitterList(Handle),
    Display(Configuration),
    Power(PowerCommand)
}

pub enum SourceEvent {
	Info(SourceInfo),
}

pub struct SourceInfo {

}

pub type Measurement = f32;

pub enum Flux {
	Relative(Measurement),
	Absolute(Measurement)
}

pub enum EmitterCommand {
	Info,
	FluxRange,
	FluxSet(Flux),
	CharacteristicSpectralDistribution(Handle, u32)
}

pub enum EmitterEvent {
	Info(EmitterInfo),
	FluxRange(u32, u32),
	FluxSet(Flux),
	CharacteristicSpectralDistribution(Handle, Vec<(f32, f32)>)
}

pub struct EmitterInfo {

}


pub trait Runtime {
	fn handle_command();
	fn handle_event();
}
