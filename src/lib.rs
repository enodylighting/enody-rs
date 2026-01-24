#![cfg_attr(not(feature = "std"), no_std)]
#![feature(new_range_api)]
#[allow(async_fn_in_trait)]

extern crate alloc;

pub mod interface;
pub mod message;
#[cfg(feature = "remote")]
pub mod remote;

pub type Identifier = uuid::Uuid;

#[cfg(feature = "std")]
pub type USBError = rusb::Error;
#[cfg(not(feature = "std"))]
pub type USBError = ();

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Error {
    Unknown,
    Debug(String),
    Unsupported,
    USB(USBError),
    Serialization,
    Busy,
    InsufficientData,
    UnexpectedResponse
}