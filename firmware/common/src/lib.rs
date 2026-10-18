//! Common library for donger firmware projects.
//!
//! This crate provides shared utilities for:
//! - USB CDC-ACM packet reading/writing with COBS framing
//! - Protocol handling for odyssey-protocol messages
//! - Stream info management for object/IMU data
//! - USB device setup with WinUSB support (nrf5340 feature)
//!
//! This is designed to be used by atslite boards with PAG766x sensors.

#![no_std]

pub mod packet;
pub mod stream;
pub mod usb;

#[cfg(feature = "atslite")]
pub mod usb_device;

// Re-exports
pub use odyssey_protocol;

/// Helper macro to create a static byte buffer
#[macro_export]
macro_rules! static_byte_buffer {
    ($size:expr_2021) => {{
        static BUFFER: static_cell::ConstStaticCell<[u8; $size]> =
            static_cell::ConstStaticCell::new([0u8; $size]);
        BUFFER.take()
    }};
}
