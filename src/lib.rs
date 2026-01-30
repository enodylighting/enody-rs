#![cfg_attr(not(feature = "std"), no_std)]
#![feature(new_range_api)]
#[allow(async_fn_in_trait)]

extern crate alloc;

#[cfg(feature = "remote")]
pub mod environment;
pub mod emitter;
pub mod fixture;
pub mod host;
pub mod source;
pub mod message;
pub mod runtime;
pub mod serialization;
pub mod spectral;
#[cfg(feature = "remote")]
pub mod usb;

pub type Identifier = uuid::Uuid;
pub type Measurement = f32;

#[cfg(feature = "std")]
pub type DebugError = String;
#[cfg(not(feature = "std"))]
pub type DebugError = heapless::String<128>;

#[cfg(feature = "std")]
pub type USBError = rusb::Error;
#[cfg(not(feature = "std"))]
pub type USBError = ();

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Error {
    Unknown,
    Debug(DebugError),
    Unsupported,
    USB(USBError),
    Serialization,
    Busy,
    InsufficientData,
    UnexpectedResponse
}