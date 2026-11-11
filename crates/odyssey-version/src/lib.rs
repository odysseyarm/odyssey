//! Release numbers that firmware publishes and the host reads.
//!
//! A leaf crate with no dependency on the protocol, so a net core or a
//! bootloader can use it without pulling in the rest of Odyssey.
#![no_std]

#[cfg(kani)]
mod proofs;
mod usb_release;

pub use usb_release::{BootloaderRelease, UsbDeviceRelease, UsbVersionReport};
