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

pub trait Recipient {
	fn handle_command(&mut self, command: CommandMessage) -> Result<(), crate::Error>;
	fn handle_event(&mut self, event: EventMessage) -> Result<(), crate::Error>;
}

pub trait Responder {
	fn handle_command(&mut self, command: CommandMessage, responder: &mut impl Recipient) -> Result<(), crate::Error>;
	fn handle_event(&mut self, event: EventMessage, responder: &mut impl Recipient) -> Result<(), crate::Error>;
}

pub trait Runtime: Responder {
}