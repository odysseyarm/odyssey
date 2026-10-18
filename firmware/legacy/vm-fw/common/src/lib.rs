#![no_std]
#![feature(never_type)]

// Platform abstraction - traits for board implementations
pub mod platform;

// Platform-agnostic core modules
pub mod device_control;
pub mod fodtrigger;
pub mod object_mode;
pub mod settings;
pub mod usb;
pub mod utils;

// Re-exports for convenience
// Type aliases for PAJ sensors
use core::convert::Infallible;

pub use embassy_nrf;
use embassy_nrf::gpio::{AnyPin, Output};
use embassy_nrf::spim::{self, Spim};
pub use embassy_sync;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
pub use embassy_time;
use embassy_time::Delay;
pub use embassy_usb;
use embedded_hal_bus::spi::{DeviceError, ExclusiveDevice};
pub use odyssey_protocol;
pub use paj7025;

pub type Paj = paj7025::Paj7025<
    ExclusiveDevice<Spim<'static>, Output<'static>, Delay>,
    DeviceError<spim::Error, Infallible>,
>;
// Generic PajGroup that works with any GPIOTE channel type
pub type PajGroup<'a, C> = (
    &'a AsyncMutex<NoopRawMutex, Paj>,
    embassy_nrf::Peri<'a, AnyPin>,
    embassy_nrf::Peri<'a, C>,
);
