#![no_std]

/// Since: Odyssey Desktop 0.7.0.
pub mod imu;
/// Since: Odyssey Desktop 0.7.0.
pub mod l2cap;
/// Since: Odyssey Desktop 0.7.0.
#[macro_use]
pub mod pins;
/// Since: Odyssey Desktop 0.7.0.
pub mod platform;
/// nRF52840 hardware setup and task assembly.
pub mod startup;
/// Since: Odyssey Desktop 0.7.0.
pub mod utils;

/// Since: Odyssey Desktop 0.7.0.
pub use embassy_executor;
/// Since: Odyssey Desktop 0.7.0.
pub use embassy_nrf;
/// Since: Odyssey Desktop 0.7.0.
pub use legacy_common;
/// Since: Odyssey Desktop 0.7.0.
pub use platform::VmPlatform;
