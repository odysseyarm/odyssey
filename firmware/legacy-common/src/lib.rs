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
use embassy_nrf::gpio::{AnyPin, Output};
use embassy_nrf::spim::{self, Spim};
/// Since: Odyssey Desktop 0.7.0.
pub use embassy_sync;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
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

/// Since: Odyssey Desktop 0.7.0.
pub type Paj = paj7025::Paj7025<
    ExclusiveDevice<Spim<'static>, Output<'static>, Delay>,
    DeviceError<spim::Error, Infallible>,
>;
/// Since: Odyssey Desktop 0.7.0.
pub type PajGroup<'a, C> = (
    &'a AsyncMutex<NoopRawMutex, Paj>,
    embassy_nrf::Peri<'a, AnyPin>,
    embassy_nrf::Peri<'a, C>,
);
