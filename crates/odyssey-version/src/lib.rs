//! Release numbers that firmware publishes and the host reads.
//!
//! A leaf crate with no dependency on the protocol, so a net core or a
//! bootloader can use it without pulling in the rest of Odyssey.
#![no_std]

#[cfg(test)]
extern crate std;

mod cargo_version;
mod legacy_bootloader_version;
#[cfg(kani)]
mod proofs;
mod usb_revision;

/// Since: Odyssey Desktop (unreleased).
pub use cargo_version::parse_cargo_version;
/// Since: Odyssey Desktop (unreleased).
pub use legacy_bootloader_version::{legacy_bootloader_counter, legacy_bootloader_version};
/// Since: Odyssey Desktop (unreleased).
pub use usb_revision::firmware_bcd_device;
