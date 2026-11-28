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
//! `DEVICE_PROTOCOL` versions packet contents, including packets nested inside
//! mux messages. `MUX_PROTOCOL` versions only the mux envelope, independently.
//!
//! The [report streaming design](https://github.com/odysseyarm/odyssey/blob/master/docs/contributing/report-streaming.md)
//! describes compact reports, USB and BLE packing, measurement timing and release compatibility.
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
/// Since: Unreleased.
pub mod ota;
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
mod transfer;
mod version;

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
/// Since: Unreleased.
///
pub mod marker_hint;
/// Since: Odyssey Desktop 0.6.0.
///
pub mod mux;
/// Since: Odyssey Desktop 0.6.0.
///
pub mod wire;

pub use config::{AccelConfig, ConfigKind, GeneralConfig, GyroConfig};
pub use device_name::{DeviceName, SetDeviceNameError, SetDeviceNameResult};
pub use error::Error;
pub use list::List;
pub use packet::{Packet, PacketData, PacketType, VendorData};
pub use product_id::ProductId;
pub use props::{PropKind, Props};
pub use register::{Port, ReadRegisterResponse, Register, WriteRegister};
pub use reports::{
    AccelBatch, AccelReport, BatteryReport, ImpactReport, MarkersReport, SparseMarkersError,
    SparseMarkersReport,
};
pub use stream_update::{StreamUpdate, StreamUpdateAction};
pub use transfer::pack_queued;
pub use version::{DEVICE_PROTOCOL, Version};
