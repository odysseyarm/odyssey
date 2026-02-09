//! ATSlite common library
//!
//! This crate provides shared functionality for ATSlite boards (legacy and lite1):
//! - Power management (NPM1300 PMIC)
//! - Power button handling
//! - LED state management via PMIC
//! - BLE stack (host, peripheral, L2CAP bridge)
//! - Transport mode switching (USB vs BLE)
//! - Device control interface
//! - nRF5340 initialization
//! - Object mode (protodongers protocol handling)

#![no_std]

pub mod ble;
pub mod device_control;
pub mod device_control_task;
pub mod nrf5340_init;
pub mod object_mode;
pub mod pmic_leds;
pub mod power;
pub mod power_button;
pub mod power_state;
pub mod settings;
pub mod transport_mode;
pub mod utils;

// Re-exports
pub use npm1300_rs;
pub use protodongers;
