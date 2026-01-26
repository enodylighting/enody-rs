use alloc::sync::Arc;
use core::range::RangeInclusive;

use crate::{
	Error,
	Identifier,
	message::{
		Configuration, Flux, Version
	},
	spectral::SpectralData
};

pub trait Host {
	fn identifier(&self) -> Identifier;
	fn version(&self) -> Version;
	fn fixtures(&self) -> &[impl Fixture];
}

pub trait Fixture {
	fn identifier(&self) -> Identifier;
    fn display(&mut self, config: Configuration, target_flux: Flux) -> Result<(Configuration, Flux), Error>;
	fn sources(&self) -> &[impl Source];
}

pub trait Source {
	fn identifier(&self) -> Identifier;
	fn display(&mut self, config: Configuration, target_flux: Flux) -> Result<(Configuration, Flux), Error>;
	fn emitters(&self) -> &[impl Emitter];
}

pub trait Emitter {
	fn identifier(&self) -> Identifier;
    fn flux_range(&self) -> RangeInclusive<Flux>;
	fn set_flux(&self, target_flux: Flux) -> Result<Flux, Error>;
	fn spectral_data(&self, target_flux: Flux) -> SpectralData;
}

pub trait Runtime<InternalCommand = (), InternalEvent = ()>  {
	fn host(&self) -> impl Host;
	fn environments(&self) -> &[impl Environment];
}

pub trait RemoteRuntime: Runtime {
}

pub trait Environment {
	fn identifier(&self) -> Identifier;
	fn runtimes(&self) -> &[Arc<impl RemoteRuntime>];
}
