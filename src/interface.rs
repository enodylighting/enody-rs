use core::range::RangeInclusive;

use crate::{
	Error,
	Identifier,
	message::Flux,
	spectral::SpectralData
};

pub trait Emitter: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn flux_range(&self) -> RangeInclusive<Flux>;
    fn set_flux(&self, target_flux: Flux) -> Result<Flux, Error>;
    fn spectral_data(&self, target_flux: Flux) -> SpectralData;
}
