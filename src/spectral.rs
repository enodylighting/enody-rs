use heapless::Vec;
use serde::{Deserialize, Serialize};

use crate::Measurement;

const DEFAULT_SAMPLE_COUNT: usize = 401; // 380nm-780nm, 1 nm interval

/// A SpectralSample stores a wavelength and a corresponding dimensionless measurement.
/// The measurement can represent transmitance, reflectance, and absorbance.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpectralSample {
    wavelength: Measurement,
    measurement: Measurement,
}

impl SpectralSample {
    pub fn new(wavelength: Measurement, measurement: Measurement) -> Self {
        Self {
            wavelength,
            measurement,
        }
    }

    pub fn wavelength(&self) -> Measurement {
        self.wavelength
    }

    pub fn measurement(&self) -> Measurement {
        self.measurement
    }
}

/// A SpectralData is a collection of SpectralSamples.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpectralData<const SAMPLE_COUNT: usize = DEFAULT_SAMPLE_COUNT> {
    samples: Vec<SpectralSample, SAMPLE_COUNT>,
}

impl<const SAMPLE_COUNT: usize> SpectralData<SAMPLE_COUNT> {
    pub fn new(samples: Vec<SpectralSample, SAMPLE_COUNT>) -> Self {
        Self { samples }
    }

    pub fn samples(&self) -> &Vec<SpectralSample, SAMPLE_COUNT> {
        &self.samples
    }
}

#[cfg(feature = "remote")]
use crate::{message::HostInfo, Identifier};

/// A snapshot of the full device hierarchy with spectral data attached to each emitter.
#[cfg(feature = "remote")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostSpectralData {
    pub host: HostInfo,
    pub fixtures: alloc::vec::Vec<FixtureSpectralData>,
}

/// Spectral data for all sources within a single fixture.
#[cfg(feature = "remote")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FixtureSpectralData {
    pub identifier: Identifier,
    pub sources: alloc::vec::Vec<SourceSpectralData>,
}

/// Spectral data for all emitters within a single source.
#[cfg(feature = "remote")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceSpectralData {
    pub identifier: Identifier,
    pub emitters: alloc::vec::Vec<EmitterSpectralData>,
}

/// Spectral data for a single emitter.
#[cfg(feature = "remote")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmitterSpectralData {
    pub identifier: Identifier,
    pub spectral_data: SpectralData,
}
