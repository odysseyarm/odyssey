#![no_std]

/// Since: Odyssey Desktop 0.7.0.
pub mod imu;
/// Since: Odyssey Desktop 0.7.0.
pub mod l2cap;
/// Since: Odyssey Desktop 0.7.0.
pub mod pins;
/// Since: Odyssey Desktop 0.7.0.
pub mod platform;
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
