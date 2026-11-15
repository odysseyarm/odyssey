//! Code shared by the ATSlite boards (legacy and lite1): NPM1300 power management, power
//! button, PMIC-driven status LEDs, the BLE stack (host, peripheral, L2CAP bridge), the device
//! control interface, nRF5340 bring-up, and object mode (odyssey-protocol packet handling over
//! USB and BLE).

#![no_std]
#![expect(
    unsafe_code,
    reason = "Board bring-up and shared hardware buffers require raw peripheral access"
)]

/// Since: Odyssey Desktop 0.7.0.
pub mod ble;
/// Since: Odyssey Desktop 0.7.1.
pub mod crash_record;
/// Since: Odyssey Desktop 0.7.0.
pub mod device_control;
/// Since: Odyssey Desktop 0.7.0.
pub mod device_control_task;
/// Since: Odyssey Desktop 0.7.0.
pub mod nrf5340_init;
/// Since: Odyssey Desktop 0.7.0.
pub mod object_mode;
/// Since: Unreleased.
pub mod ota;
/// Since: Odyssey Desktop 0.7.0.
pub mod pmic_leds;
/// Since: Odyssey Desktop 0.7.0.
pub mod power;
/// Since: Odyssey Desktop 0.7.0.
pub mod power_button;
/// Since: Odyssey Desktop 0.7.0.
pub mod power_state;
/// Since: Odyssey Desktop 0.7.0.
pub mod settings;
/// Since: Odyssey Desktop 0.7.0.
pub mod utils;

/// Since: Odyssey Desktop 0.7.0.
pub use npm1300_rs;
/// Since: Odyssey Desktop 0.7.0.
pub use odyssey_protocol;
