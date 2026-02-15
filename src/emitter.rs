use core::ops::RangeInclusive;

use crate::{message::Flux, spectral::SpectralData, Error, Identifier};

pub trait Emitter: Send + Sync {
    fn identifier(&self) -> Identifier;
    fn flux_range(&self) -> RangeInclusive<Flux>;
    fn set_flux(&self, target_flux: Flux) -> Result<Flux, Error>;
    fn spectral_data(&self, target_flux: Flux) -> SpectralData;
}
