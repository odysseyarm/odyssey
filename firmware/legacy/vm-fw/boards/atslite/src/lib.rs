#![no_std]

pub mod battery_model;
pub mod imu;
pub mod l2cap;
pub mod pins;
pub mod platform;

pub use atslite_common::{
    ble, device_control, device_control_task, nrf5340_init, pmic_leds, power, power_button,
    power_state, transport_mode, utils,
};
// vm-fw common: object mode, settings and the rest of the board-independent code.
pub use common;
pub use embassy_executor;
pub use embassy_nrf;
use embassy_nrf::{bind_interrupts, cryptocell_rng, interrupt, pac, peripherals, spim, twim};
pub use platform::AtslitePlatform;

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
