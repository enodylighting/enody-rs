use alloc::boxed::Box;
use core::range::RangeInclusive;

use crate::{
	Error,
	Identifier,
	message::{
		Configuration, Flux
	},
	spectral::SpectralData
};

pub trait Fixture: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn display(&mut self, config: Configuration, target_flux: Flux) -> Result<(Configuration, Flux), Error>;
    fn sources(&self) -> &[Box<dyn Source>];
}

pub trait Source: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn display(&mut self, config: Configuration, target_flux: Flux) -> Result<(Configuration, Flux), Error>;
    fn emitters(&self) -> &[Box<dyn Emitter>];
}

pub trait Emitter: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn flux_range(&self) -> RangeInclusive<Flux>;
    fn set_flux(&self, target_flux: Flux) -> Result<Flux, Error>;
    fn spectral_data(&self, target_flux: Flux) -> SpectralData;
}
