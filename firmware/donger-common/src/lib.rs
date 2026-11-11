//! Code shared by the donger firmware for the atslite boards with PAG766x
//! sensors: COBS framing (`packet`), CDC-ACM serial helpers (`usb`),
//! per-stream enable state (`stream`), a re-export of odyssey-protocol and,
//! with the `atslite` feature, nRF5340 USB device setup with WinUSB support
//! (`usb_device`).

#![no_std]
#![expect(
    unsafe_code,
    reason = "USB serial descriptor conversion uses a checked ASCII-hex buffer"
)]

/// Since: Odyssey Desktop 0.7.0.
pub mod packet;
/// Since: Odyssey Desktop 0.7.0.
pub mod stream;
/// Since: Odyssey Desktop 0.7.0.
pub mod usb;

/// Since: Odyssey Desktop 0.7.0.
#[cfg(feature = "atslite")]
pub mod usb_device;

/// Since: Odyssey Desktop 0.7.0.
pub use odyssey_protocol;

/// Expands to a zeroed `&'static mut [u8; $size]` backed by a `ConstStaticCell`.
/// Each expansion site owns one buffer and panics if evaluated a second time.
///
/// Since: Odyssey Desktop 0.7.0.
#[macro_export]
macro_rules! static_byte_buffer {
    ($size:expr_2021) => {{
        static BUFFER: static_cell::ConstStaticCell<[u8; $size]> =
            static_cell::ConstStaticCell::new([0u8; $size]);
        BUFFER.take()
    }};
}
