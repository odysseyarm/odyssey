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

use embassy_nrf::gpio::{AnyPin, Output};
use embassy_nrf::spim::{self, Spim};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::Delay;
use embedded_hal_bus::spi::{DeviceError, ExclusiveDevice};
pub use {embassy_nrf, embassy_sync, embassy_time, embassy_usb, paj7025, protodongers};

pub type Paj =
    paj7025::Paj7025<ExclusiveDevice<Spim<'static>, Output<'static>, Delay>, DeviceError<spim::Error, Infallible>>;
// Generic PajGroup that works with any GPIOTE channel type
pub type PajGroup<'a, C> = (
    &'a AsyncMutex<NoopRawMutex, Paj>,
    embassy_nrf::Peri<'a, AnyPin>,
    embassy_nrf::Peri<'a, C>,
);
