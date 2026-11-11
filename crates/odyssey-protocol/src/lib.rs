//! Device, dongle-control, mux and accessory wire schemas.
//!
//! `Since` dates Rust API availability by tagged Desktop release. A variant's
//! `Wire baseline` is the protocol counter in that first tagged snapshot. It
//! promises no minimum peer version: explicit minimum-version contracts and
//! runtime capability checks stay authoritative. Independent endpoint counters
//! from before protocol-version unification are not comparable. The marker Mode
//! characteristic has no wire-version counter of its own.
//!
//! Availability metadata changes no discriminants, payloads or negotiation.
// Assumes a little-endian target.
#![no_std]

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "minicbor")]
mod cbor;
mod config;
mod device_name;
mod error;
mod list;
mod packet;
mod product_id;
#[cfg(kani)]
mod proofs;
mod props;
#[cfg(feature = "pyo3")]
mod py;
mod register;
mod reports;
#[cfg(feature = "schema")]
mod schema;
#[cfg(all(feature = "serde", feature = "device", not(feature = "std")))]
mod serde_reject;
mod stream_update;
mod version;
/// Since: Unreleased.
pub mod ota;

/// Since: Odyssey Desktop 0.7.4.
///
pub mod accessory;
/// Since: Odyssey Desktop 0.6.0.
///
pub mod control;
/// Since: Odyssey Desktop 0.7.0.
///
pub mod image;
/// Since: Odyssey Desktop 0.7.5.
///
pub mod legacy;
/// Since: Odyssey Desktop 0.7.4.
///
pub mod marker_control;
/// Since: Odyssey Desktop 0.6.0.
///
pub mod mux;
/// Since: Odyssey Desktop 0.6.0.
///
pub mod wire;

/// Since: Odyssey Desktop 0.6.0.
pub use config::{AccelConfig, ConfigKind, GeneralConfig, GyroConfig};
/// Since: Odyssey Desktop 0.6.0.
pub use device_name::{DeviceName, SetDeviceNameError, SetDeviceNameResult};
/// Since: Odyssey Desktop 0.6.0.
pub use error::Error;
/// Since: Odyssey Desktop 0.6.0.
pub use list::List;
/// Since: Odyssey Desktop 0.6.0.
pub use packet::{Packet, PacketData, PacketType, VendorData};
/// Since: Odyssey Desktop 0.6.0.
pub use product_id::ProductId;
/// Since: Odyssey Desktop 0.6.0.
pub use props::{PropKind, Props};
/// Since: Odyssey Desktop 0.6.0.
pub use register::{Port, ReadRegisterResponse, Register, WriteRegister};
/// Device event payloads.
///
/// # Since
///
/// - `AccelReport`: Odyssey Desktop 0.6.0.
/// - `BatteryReport`: Odyssey Desktop 0.6.0.
/// - `ImpactReport`: Odyssey Desktop 0.6.0.
/// - `MarkersReport`: Odyssey Desktop 0.7.5.
pub use reports::{AccelReport, BatteryReport, ImpactReport, MarkersReport};
/// Since: Odyssey Desktop 0.6.0.
pub use stream_update::{StreamUpdate, StreamUpdateAction};
/// Since: Odyssey Desktop 0.6.0.
pub use version::{DEVICE_PROTOCOL, Version};
