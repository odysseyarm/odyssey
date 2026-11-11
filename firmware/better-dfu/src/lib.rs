#![no_std]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
mod fmt;

/// Since: Odyssey Desktop 0.6.0.
pub mod consts;

/// Since: Odyssey Desktop 0.6.0.
#[cfg(feature = "dfu")]
pub mod dfu;
/// Since: Odyssey Desktop 0.6.0.
#[cfg(all(feature = "dfu", not(feature = "application")))]
pub use self::dfu::*;

/// Since: Odyssey Desktop 0.6.0.
#[cfg(feature = "application")]
pub mod application;
/// Since: Odyssey Desktop 0.6.0.
#[cfg(all(feature = "application", not(feature = "dfu")))]
pub use self::application::*;

/// System reset issued after DETACH and after the manifestation phase.
///
/// With the `cortex-m` or `esp32c3-hal` feature the crate provides `ResetImmediate`, which issues the reset
/// without touching any other peripheral. For other behaviour, pass a custom implementation to `Control::new`.
///
/// Since: Odyssey Desktop 0.6.0.
pub trait Reset {
    /// Reset the device.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    fn sys_reset(&self);
}

/// Reset immediately.
///
/// Since: Odyssey Desktop 0.6.0.
#[cfg(feature = "esp32c3-hal")]
pub struct ResetImmediate;

#[cfg(feature = "esp32c3-hal")]
impl Reset for ResetImmediate {
    fn sys_reset(&self) {
        esp32c3_hal::reset::software_reset();
        loop {}
    }
}

/// Reset immediately.
///
/// Since: Odyssey Desktop 0.6.0.
#[cfg(feature = "cortex-m")]
pub struct ResetImmediate;

#[cfg(feature = "cortex-m")]
impl Reset for ResetImmediate {
    fn sys_reset(&self) {
        cortex_m::peripheral::SCB::sys_reset()
    }
}
