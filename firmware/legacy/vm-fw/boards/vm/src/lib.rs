#![no_std]

// VM board implementation modules
pub mod imu;
pub mod l2cap;
pub mod pins;
pub mod platform;
pub mod utils;

// Re-export common
pub use common;
// Re-export embassy for convenience
pub use embassy_executor;
pub use embassy_nrf;
// Re-export platform implementation
pub use platform::VmPlatform;
