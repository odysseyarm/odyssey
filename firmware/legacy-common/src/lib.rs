#![no_std]
#![expect(
    unsafe_code,
    reason = "Legacy register setup and cfg-noodle callbacks require raw peripheral access"
)]

/// Since: Odyssey Desktop 0.7.0.
pub mod platform;

/// Since: Odyssey Desktop 0.7.0.
pub mod device_control;
/// Since: Odyssey Desktop 0.7.0.
pub mod fodtrigger;
/// Since: Odyssey Desktop 0.7.0.
pub mod image_mode;
/// Since: Odyssey Desktop 0.7.0.
pub mod object_mode;
/// Since: Odyssey Desktop 0.7.0.
pub mod settings;
/// Since: Odyssey Desktop 0.7.0.
pub mod usb;
/// Since: Odyssey Desktop 0.7.0.
pub mod utils;

use core::convert::Infallible;

/// Since: Odyssey Desktop 0.7.0.
pub use embassy_nrf;
use embassy_nrf::gpio::Output;
use embassy_nrf::mode::Async;
use embassy_nrf::spim::{self, Spim};
/// Since: Odyssey Desktop 0.7.0.
pub use embassy_sync;
/// Since: Odyssey Desktop 0.7.0.
pub use embassy_time;
use embassy_time::Delay;
/// Since: Odyssey Desktop 0.7.0.
pub use embassy_usb;
use embedded_hal_bus::spi::{DeviceError, ExclusiveDevice};
/// Since: Odyssey Desktop 0.7.0.
pub use odyssey_protocol;
/// Since: Odyssey Desktop 0.7.0.
pub use paj7025;

/// A PAJ7025 on a register SPI bus that borrows its peripheral and pins for `'d`.
///
/// Since: Odyssey Desktop 0.7.0.
pub type Paj<'d> = paj7025::Paj7025<
    ExclusiveDevice<Spim<'d, Async>, Output<'d>, Delay>,
    DeviceError<spim::Error, Infallible>,
>;
