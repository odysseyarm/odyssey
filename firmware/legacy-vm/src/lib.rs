#![no_std]

pub mod imu;
pub mod l2cap;
pub mod pins;
pub mod platform;
pub mod utils;

pub use embassy_executor;
pub use embassy_nrf;
pub use legacy_common;
pub use platform::VmPlatform;
