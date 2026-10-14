#![no_std]
#![feature(never_type)]

// VM board implementation modules
pub mod imu;
pub mod l2cap;
pub mod pins;
pub mod platform;
pub mod utils;

// Re-export common
pub use common;

// Re-export platform implementation
pub use platform::VmPlatform;

// Re-export embassy for convenience
pub use embassy_executor;
pub use embassy_nrf;
