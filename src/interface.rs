use core::range::RangeInclusive;

use crate::{
	Identifier,
	message::{
		Flux,
		Version
	}
};

pub trait Host {
	fn version(&self) -> Version;
    fn identifier(&self) -> Identifier;
}

pub trait Runtime {
	fn host(&self) -> impl Host;
	fn environments(&self) -> Vec<impl Environment>;
}

pub trait Environment {
	
}

pub trait Emitter {
	fn identifier(&self) -> Identifier;
    fn flux_range(&self) -> RangeInclusive<Flux>;
}
