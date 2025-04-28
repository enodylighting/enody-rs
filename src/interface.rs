use crate::Identifier;

pub trait Emitter<Config, Meas> {
	fn identifier(&self) -> Identifier;
    // fn flux_range(&self) -> RangeInclusive<Flux<Meas>>;
	// fn characteristic_spectral_distribution(&self) -> SpectralData<Flux<Meas>>;
}
