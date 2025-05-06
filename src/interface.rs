use crate::{
	Identifier,
	message::{
		CommandMessage,
		EventMessage
	}
};

pub trait Emitter<Config, Meas> {
	fn identifier(&self) -> Identifier;
    // fn flux_range(&self) -> RangeInclusive<Flux<Meas>>;
	// fn characteristic_spectral_distribution(&self) -> SpectralData<Flux<Meas>>;
}

pub trait Recipient<InternalCommand, InternalEvent> {
	fn handle_command(&mut self, command: CommandMessage<InternalCommand>) -> Result<(), crate::Error>;
	fn handle_event(&mut self, event: EventMessage<InternalEvent>) -> Result<(), crate::Error>;
}

pub trait Responder<InternalCommand, InternalEvent> {
	fn handle_command(&mut self, command: CommandMessage<InternalCommand>, responder: &mut impl Recipient<InternalCommand, InternalEvent>) -> Result<(), crate::Error>;
	fn handle_event(&mut self, event: EventMessage<InternalEvent>, responder: &mut impl Recipient<InternalCommand, InternalEvent>) -> Result<(), crate::Error>;
}

pub trait Runtime<InternalCommand, InternalEvent>: Responder<InternalCommand, InternalEvent> {
	
}