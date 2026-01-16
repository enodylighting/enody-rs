use alloc::sync::Arc;
use core::range::RangeInclusive;

use crate::{
	Error, Identifier, message::{
		Configuration, Flux, SpectralSample, Version
	}
};

pub trait Host {
	fn identifier(&self) -> Identifier;
	fn version(&self) -> Version;
	fn fixtures(&self) -> Vec<impl Fixture>;
}

pub trait Fixture {
	fn identifier(&self) -> Identifier;
    fn display(&mut self, config: Configuration, target_flux: Flux) -> Result<(Configuration, Flux), Error>;
	fn sources(&self) -> Vec<impl Source>;
}

pub trait Source {
	fn identifier(&self) -> Identifier;
	fn display(&mut self, config: Configuration, target_flux: Flux) -> Result<(Configuration, Flux), Error>;
	fn emitters(&self) -> Vec<impl Emitter>;
}

pub trait Emitter {
	fn identifier(&self) -> Identifier;
    fn flux_range(&self) -> RangeInclusive<Flux>;
	fn set_flux(&self, target_flux: Flux) -> Result<Flux, Error>;
	fn spectral_data(&self, target_flux: Flux) -> SpectralData;
}

pub struct SpectralData {
	_samples: Vec<SpectralSample>
}

pub trait Runtime<InternalCommand = (), InternalEvent = ()>  {
	fn host(&self) -> impl Host;
	fn environments(&self) -> Vec<impl Environment>;
}

pub trait RemoteRuntime: Runtime {
}

pub trait Environment {
	fn identifier(&self) -> Identifier;
	fn runtimes(&self) -> Vec<Arc<impl RemoteRuntime>>;
}
