//! Board support for lite1: the PAG7665QN vision sensor and nPM1300 PMIC.

#![no_std]

/// Since: Odyssey Desktop 0.6.0.
pub mod battery_model;
/// Since: Odyssey Desktop 0.6.0.
pub mod imu;
/// Since: Odyssey Desktop 0.6.0.
pub mod init;
/// Since: Odyssey Desktop 0.6.0.
pub mod pins;
/// Since: Odyssey Desktop 0.6.0.
pub mod sensors;
/// Since: Odyssey Desktop 0.6.0.
pub mod usb;

/// Since: Odyssey Desktop 0.6.0.
pub use atslite_common::{
    ble, device_control, device_control_task, nrf5340_init, object_mode, pmic_leds, power,
    power_button, power_state, settings, utils,
};
/// Since: Odyssey Desktop 0.6.0.
pub use embassy_executor;
/// Since: Odyssey Desktop 0.6.0.
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

/// Returns a `&'static mut [u8; $size]` zeroed buffer; each expansion site can take it once.
///
/// Since: Odyssey Desktop 0.6.0.
#[macro_export]
macro_rules! static_byte_buffer {
    ($size:expr) => {{
        static BUFFER: static_cell::ConstStaticCell<[u8; $size]> =
            static_cell::ConstStaticCell::new([0u8; $size]);
        BUFFER.take()
    }};
}
