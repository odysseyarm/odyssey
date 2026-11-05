//! Code shared by the ATSlite boards (legacy and lite1): NPM1300 power management, power
//! button, PMIC-driven status LEDs, the BLE stack (host, peripheral, L2CAP bridge), the device
//! control interface, nRF5340 bring-up, and object mode (odyssey-protocol packet handling over
//! USB and BLE).

#![no_std]

pub mod ble;
pub mod crash_record;
pub mod device_control;
pub mod device_control_task;
pub mod nrf5340_init;
pub mod object_mode;
pub mod pmic_leds;
pub mod power;
pub mod power_button;
pub mod power_state;
pub mod settings;
pub mod utils;

pub use npm1300_rs;
pub use odyssey_protocol;
