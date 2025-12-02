#![no_std]
#![feature(never_type)]

// Platform abstraction - traits for board implementations
pub mod platform;

// Platform-agnostic core modules
pub mod utils;
pub mod fodtrigger;
pub mod object_mode;
pub mod settings;
pub mod usb;

// Re-exports for convenience
pub use embassy_nrf;
pub use embassy_sync;
pub use embassy_time;
pub use embassy_usb;
pub use paj7025;
pub use protodongers;

// Type aliases for PAJ sensors
use core::convert::Infallible;

use embassy_nrf::gpio::{AnyPin, Output};
use embassy_nrf::gpiote::AnyChannel;
use embassy_nrf::spim::{self, Spim};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::Delay;
use embedded_hal_bus::spi::{DeviceError, ExclusiveDevice};

pub type Paj = paj7025::Paj7025<
    ExclusiveDevice<Spim<'static>, Output<'static>, Delay>,
    DeviceError<spim::Error, Infallible>,
>;
pub type PajGroup<'a> = (
    &'a AsyncMutex<NoopRawMutex, Paj>,
    embassy_nrf::Peri<'a, AnyPin>,
    embassy_nrf::Peri<'a, AnyChannel>,
);
