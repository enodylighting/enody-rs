//! Spectral sample and spectral data structures.
//!
//! Spectral data is represented as wavelength/measurement samples. The default
//! capacity covers 380 nm through 780 nm at 1 nm intervals.

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
    /// Creates a spectral sample from a wavelength and measurement.
    pub fn new(wavelength: Measurement, measurement: Measurement) -> Self {
        Self {
            wavelength,
            measurement,
        }
    }

    /// Returns the sample wavelength.
    pub fn wavelength(&self) -> Measurement {
        self.wavelength
    }

    /// Returns the sample measurement value.
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
    /// Creates spectral data from a fixed-capacity sample vector.
    pub fn new(samples: Vec<SpectralSample, SAMPLE_COUNT>) -> Self {
        Self { samples }
    }

    /// Returns all samples in this spectral data set.
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
    /// Host metadata.
    pub host: HostInfo,
    /// Fixture-level spectral data.
    pub fixtures: alloc::vec::Vec<FixtureSpectralData>,
}

/// Spectral data for all sources within a single fixture.
#[cfg(feature = "remote")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FixtureSpectralData {
    /// Fixture identifier.
    pub identifier: Identifier,
    /// Source-level spectral data.
    pub sources: alloc::vec::Vec<SourceSpectralData>,
}

/// Spectral data for all emitters within a single source.
#[cfg(feature = "remote")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceSpectralData {
    /// Source identifier.
    pub identifier: Identifier,
    /// Emitter-level spectral data.
    pub emitters: alloc::vec::Vec<EmitterSpectralData>,
}

/// Spectral data for a single emitter.
#[cfg(feature = "remote")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmitterSpectralData {
    /// Emitter identifier.
    pub identifier: Identifier,
    /// Downloaded spectral data for the emitter.
    pub spectral_data: SpectralData,
}
