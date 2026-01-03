#![cfg_attr(not(feature = "std"), no_std)]
#![feature(new_range_api)]

pub mod interface;
pub mod message;
#[cfg(feature = "remote")]
pub mod remote;

pub type Identifier = uuid::Uuid;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Error {
    Unknown,
    Debug(String),
    Unsupported,
    #[cfg(feature = "std")]
    USB(rusb::Error),
    Serialization,
    Busy,
    InsufficientData
}