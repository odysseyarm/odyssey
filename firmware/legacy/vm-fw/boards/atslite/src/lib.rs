#![no_std]
#![feature(never_type)]

// ATSlite board implementation modules - board-specific
pub mod battery_model;
pub mod imu;
pub mod l2cap;
pub mod pins;
pub mod platform;

// Re-export from atslite-common
pub use atslite_common::ble;
pub use atslite_common::device_control;
pub use atslite_common::device_control_task;
pub use atslite_common::nrf5340_init;
pub use atslite_common::pmic_leds;
pub use atslite_common::power;
pub use atslite_common::power_button;
pub use atslite_common::power_state;
pub use atslite_common::transport_mode;
pub use atslite_common::utils;

// Re-export common (vm-fw common for object mode, settings, etc.)
pub use common;

// Re-export platform implementation
pub use platform::AtslitePlatform;

// Re-export embassy for convenience
pub use embassy_executor;
pub use embassy_nrf;

use embassy_nrf::{bind_interrupts, cryptocell_rng, interrupt, pac, peripherals, spim, twim};

bind_interrupts!(pub struct Irqs {
    SERIAL0 => spim::InterruptHandler<peripherals::SERIAL0>;
    SERIAL1 => spim::InterruptHandler<peripherals::SERIAL1>;
    SERIAL2 => twim::InterruptHandler<peripherals::SERIAL2>;
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TIMER0 => TimerIrq;
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
    CRYPTOCELL => cryptocell_rng::InterruptHandler<peripherals::CC_RNG>;
});

pub struct TimerIrq;

impl
    interrupt::typelevel::Handler<
        <embassy_nrf::peripherals::TIMER0 as embassy_nrf::timer::Instance>::Interrupt,
    > for TimerIrq
{
    unsafe fn on_interrupt() {
        unsafe {
            let regs = embassy_nrf::pac::timer::Timer::from_ptr(pac::TIMER0.as_ptr());
            regs.events_compare(1).write(|w| *w = 0);
        }
        crate::common::fodtrigger::FOD_TICK_SIG.signal(());
    }
}
