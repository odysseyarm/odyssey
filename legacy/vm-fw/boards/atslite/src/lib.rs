#![no_std]
#![feature(never_type)]

// ATSlite board implementation modules
pub mod ble;
pub mod imu;
pub mod l2cap;
pub mod pins;
pub mod platform;
pub mod utils;

// ATSlite-specific modules
pub mod battery_model;
pub mod nrf5340_init;
pub mod pmic_leds;
pub mod power;
pub mod power_button;
pub mod device_control_task;
pub mod power_state;
pub mod transport_mode;

// Re-export common
pub use common;

// Re-export platform implementation
pub use platform::AtslitePlatform;

// Re-export embassy for convenience
pub use embassy_executor;
pub use embassy_nrf;

use embassy_nrf::{bind_interrupts, interrupt, pac, peripherals, spim, twim};

bind_interrupts!(pub struct Irqs {
    SERIAL0 => spim::InterruptHandler<peripherals::SERIAL0>;
    SERIAL1 => spim::InterruptHandler<peripherals::SERIAL1>;
    SERIAL2 => twim::InterruptHandler<peripherals::SERIAL2>;
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TIMER0 => TimerIrq;
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
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
