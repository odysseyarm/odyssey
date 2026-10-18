//! Lite1 board support library
//!
//! This module provides the board-specific functionality for the lite1 board,
//! which uses the PAG7665QN vision sensor and NPM1300 PMIC.

#![no_std]

// Board-specific modules
pub mod battery_model;
pub mod imu;
pub mod init;
pub mod pins;
pub mod sensors;
pub mod usb;

// Re-export from atslite-common
pub use atslite_common::{
    ble, device_control, device_control_task, nrf5340_init, object_mode, pmic_leds, power,
    power_button, power_state, settings, transport_mode, utils,
};
// Re-export embassy for convenience
pub use embassy_executor;
pub use embassy_nrf;
use embassy_nrf::{bind_interrupts, cryptocell_rng, peripherals, spim, twim};

bind_interrupts!(pub struct Irqs {
    SERIAL0 => spim::InterruptHandler<peripherals::SERIAL0>;
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    SERIAL2 => twim::InterruptHandler<peripherals::SERIAL2>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
    CRYPTOCELL => cryptocell_rng::InterruptHandler<peripherals::CC_RNG>;
});

/// Helper macro to create a static byte buffer
#[macro_export]
macro_rules! static_byte_buffer {
    ($size:expr) => {{
        static BUFFER: static_cell::ConstStaticCell<[u8; $size]> =
            static_cell::ConstStaticCell::new([0u8; $size]);
        BUFFER.take()
    }};
}
