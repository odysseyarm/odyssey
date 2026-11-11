//! Release numbers that firmware publishes and the host reads.
//!
//! A leaf crate with no dependency on the protocol, so a net core or a
//! bootloader can use it without pulling in the rest of Odyssey.
#![no_std]

#[cfg(test)]
extern crate std;

mod cargo_version;
#[cfg(kani)]
mod proofs;
mod usb_release;

pub use cargo_version::parse_cargo_version;
pub use usb_release::{BootloaderRelease, UsbDeviceRelease, UsbVersionReport};
