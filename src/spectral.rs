use heapless::Vec;
use serde::{ Deserialize, Serialize };

use crate::Measurement;

const DEFAULT_SAMPLE_COUNT: usize = 41; // 380nm-780nm, 10 nm interval

/// A SpectralSample stores a wavelength and a corresponding dimensionless measurement.
/// The measurement can represent transmitance, reflectance, and absorbance.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpectralSample {
	wavelength: Measurement,
	measurement: Measurement
}

impl SpectralSample {
    pub fn wavelength(&self) -> Measurement {
        self.wavelength
    }

    pub fn measurement(&self) -> Measurement {
        self.measurement
    }
}

pub struct SpectralData<const SAMPLE_COUNT: usize = DEFAULT_SAMPLE_COUNT> {
	samples: Vec<SpectralSample, SAMPLE_COUNT>
}

impl<const SAMPLE_COUNT: usize> SpectralData<SAMPLE_COUNT> {
    pub fn samples(&self) -> &Vec<SpectralSample, SAMPLE_COUNT> {
        &self.samples
    }

    // pub fn samples(&self) -> [SpectralSample<Measurement>; SPECTRAL_RESOLUTION] {
    //     let mut samples = [SpectralSample::default(); SPECTRAL_RESOLUTION];
    //     for i in 0..self.sample_count() {
    //         samples[i].wavelength = self.wavelengths[i];
    //         samples[i].measurement = self.measurements[i];
    //     }
    //     samples
    // }

    // pub fn wavelengths(&self) -> &Vec<Wavelength, SPECTRAL_RESOLUTION> {
    //     &self.wavelengths
    // }

    // pub fn measurements(&self) -> &Vec<Measurement, SPECTRAL_RESOLUTION> {
    //     &self.measurements
    // }
}
