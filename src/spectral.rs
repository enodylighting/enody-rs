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
