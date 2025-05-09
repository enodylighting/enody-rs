pub type Identifier = uuid::Uuid;

#[derive(Clone, Debug, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct Chromaticity {
    pub x: f32,
    pub y: f32
}

#[derive(Clone, Debug, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
pub enum Configuration {
	Blackbody,
	Chromatic(Chromaticity),
    Flux,
	Spectral,
    Manual
}

pub mod interface;
pub mod message;
pub mod remote;

#[derive(Debug)]
pub enum Error {
    Unknown,
    Debug(&'static str),
    Unsupported,
    USB(rusb::Error),
    Serialization,
    Busy,
    InsufficientData
}