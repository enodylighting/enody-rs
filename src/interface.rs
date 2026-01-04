use core::range::RangeInclusive;

use crate::{
	Identifier,
	message::{
		CommandMessage,
		EventMessage,
		Flux,
		Version
	}
};

pub trait Recipient<InternalCommand, InternalEvent> {
	fn handle_command(&mut self, command: CommandMessage<InternalCommand>) -> Result<(), crate::Error>;
	fn handle_event(&mut self, event: EventMessage<InternalEvent>) -> Result<(), crate::Error>;
}

pub trait Responder<InternalCommand, InternalEvent> {
	fn handle_command(&mut self, command: CommandMessage<InternalCommand>, responder: &mut impl Recipient<InternalCommand, InternalEvent>) -> Result<(), crate::Error>;
	fn handle_event(&mut self, event: EventMessage<InternalEvent>, responder: &mut impl Recipient<InternalCommand, InternalEvent>) -> Result<(), crate::Error>;
}

pub trait Host {
	fn version(&self) -> Version;
    fn identifier(&self) -> Identifier;
}

pub trait Emitter {
	fn identifier(&self) -> Identifier;
    fn flux_range(&self) -> RangeInclusive<Flux>;
}

pub trait Runtime<InternalCommand, InternalEvent>: Responder<InternalCommand, InternalEvent> {
	
}