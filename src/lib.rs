//! Rust SDK for discovering, configuring, and controlling Enody lighting products.
//!
//! The crate is split between a host-side remote SDK and a smaller `no_std`
//! protocol/trait surface. With default features enabled, applications can
//! discover devices over USB or WiFi, connect to a `runtime::remote::RemoteRuntime`,
//! and traverse the product hierarchy:
//!
//! ```text
//! Environment -> RemoteRuntime -> RemoteHost -> RemoteFixture -> RemoteSource -> RemoteEmitter
//! ```
//!
//! For embedded or constrained integrations, disable default features to keep
//! the shared message protocol, local traits, spectral data structures, and
//! WiFi wire frames available without `std`.
//!
//! # Discovery
//!
//! USB and WiFi discovery are exposed through `environment::Environment`.
//! Remote traversal is count-then-index based: hosts contain fixtures, fixtures
//! contain sources, and sources contain emitters.
//!
//! ```no_run
//! # #[cfg(feature = "remote")]
//! # async fn example() -> Result<(), enody::Error> {
//! use enody::{environment::Environment, usb::UsbEnvironment};
//!
//! let environment = UsbEnvironment::new();
//! for runtime in environment.runtimes() {
//!     let host = runtime.host().await?;
//!     for fixture in host.fixtures().await? {
//!         for source in fixture.sources().await? {
//!             let emitters = source.emitters().await?;
//!             println!("source {} has {} emitters", source.identifier(), emitters.len());
//!         }
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Light Control
//!
//! Display commands use [`message::Configuration`] and [`message::Flux`].
//!
//! ```no_run
//! # #[cfg(feature = "remote")]
//! # async fn example(fixture: enody::fixture::remote::RemoteFixture) -> Result<(), enody::Error> {
//! use enody::message::{Configuration, Flux};
//!
//! fixture
//!     .display(Configuration::Blackbody(4000.0), Flux::Relative(0.5))
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Feature Flags
//!
//! - `remote`: host-side discovery and transport APIs. This implies `std`.
//! - `cli`: the `enody` command-line tool and token persistence helpers.
//! - `std`: standard library support for shared types that can also build in
//!   `no_std` mode.
//!
//! Default features are `remote` and `cli`.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

/// Emitter traits and remote emitter handles.
pub mod emitter;
#[cfg(feature = "remote")]
#[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
/// Discovery traits and runtime arrival/removal events.
pub mod environment;
/// Fixture traits and remote fixture handles.
pub mod fixture;
/// Host traits and remote host handles.
pub mod host;
/// Public command, event, topology, setting, network, and token protocol types.
pub mod message;
/// Local runtime traits and host-side remote command dispatch.
pub mod runtime;
/// Postcard serialization plus USB STX/ETX/DLE framing helpers.
pub mod serialization;
/// Source traits and remote source handles.
pub mod source;
/// Spectral sample and spectral data containers.
pub mod spectral;
#[cfg(all(feature = "std", feature = "cli"))]
#[cfg_attr(docsrs, doc(cfg(all(feature = "std", feature = "cli"))))]
/// Host-side persistence for saved WiFi authentication tokens.
pub mod token_store;
#[cfg(feature = "remote")]
#[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
/// EP01 firmware update helpers.
pub mod update;
#[cfg(feature = "remote")]
#[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
/// USB discovery and transport backends.
pub mod usb;
/// WiFi wire protocol, discovery, authentication, and pairing helpers.
pub mod wifi;
/// Unique identifier used for hosts, fixtures, sources, emitters, commands, and events.
pub type Identifier = uuid::Uuid;
/// Floating-point measurement used for flux, chromaticity, wavelength, and spectral values.
pub type Measurement = f32;

#[cfg(feature = "std")]
/// Debug error payload when `std` is available.
pub type DebugError = String;
#[cfg(not(feature = "std"))]
/// Fixed-capacity debug error payload used in `no_std` builds.
pub type DebugError = heapless::String<128>;

#[cfg(feature = "std")]
/// USB error payload when `std` is available.
pub type USBError = String;
#[cfg(not(feature = "std"))]
/// USB error payload used in `no_std` builds where USB transports are absent.
pub type USBError = ();

/// Error type shared by local traits, wire protocol operations, and remote transports.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Error {
    /// Unclassified error.
    Unknown,
    /// Human-readable diagnostic detail.
    Debug(DebugError),
    /// Requested operation is not supported by the runtime or device.
    Unsupported,
    /// USB transport or permission failure.
    USB(USBError),
    /// Serialization or deserialization failed.
    Serialization,
    /// Resource is busy or not connected.
    Busy,
    /// Data was missing, truncated, or otherwise insufficient.
    InsufficientData,
    /// A response did not match the command that requested it.
    UnexpectedResponse,
    /// Operation timed out.
    Timeout,
    /// Caller supplied an invalid argument.
    Argument,
    /// Caller lacks permission for the requested data or operation.
    Permission,
    /// Operation was canceled.
    Canceled,
}
