#![expect(
    unsafe_code,
    reason = "timer interrupt handler accesses the TIMER0 peripheral registers"
)]
#![no_std]

/// Since: Odyssey Desktop 0.7.0.
pub mod battery_model;
/// Since: Odyssey Desktop 0.7.0.
pub mod imu;
/// Since: Odyssey Desktop 0.7.0.
pub mod l2cap;
/// Since: Odyssey Desktop 0.7.0.
pub mod pins;
/// Since: Odyssey Desktop 0.7.0.
pub mod platform;

/// Since: Odyssey Desktop 0.7.0.
pub use atslite_common::{
    ble, device_control, device_control_task, nrf5340_init, pmic_leds, power, power_button,
    power_state, utils,
};
/// Since: Odyssey Desktop 0.7.0.
pub use embassy_executor;
/// Since: Odyssey Desktop 0.7.0.
pub use embassy_nrf;
use embassy_nrf::{bind_interrupts, cryptocell_rng, interrupt, pac, peripherals, spim, twim};
// legacy-common: object mode, settings and the rest of the board-independent code.
/// Since: Odyssey Desktop 0.7.0.
pub use legacy_common;
/// Since: Odyssey Desktop 0.7.0.
pub use platform::AtslitePlatform;

bind_interrupts!(pub struct Irqs {
    SERIAL0 => spim::InterruptHandler<peripherals::SERIAL0>, embassy_nrf::spis::InterruptHandler<peripherals::SERIAL0>;
    SERIAL1 => spim::InterruptHandler<peripherals::SERIAL1>, embassy_nrf::spis::InterruptHandler<peripherals::SERIAL1>;
    SERIAL2 => twim::InterruptHandler<peripherals::SERIAL2>;
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TIMER0 => TimerIrq;
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
    CRYPTOCELL => cryptocell_rng::InterruptHandler<peripherals::CC_RNG>;
});

/// Since: Odyssey Desktop 0.7.0.
pub struct TimerIrq;

impl
    interrupt::typelevel::Handler<
        <embassy_nrf::peripherals::TIMER0 as embassy_nrf::timer::Instance>::Interrupt,
    > for TimerIrq
{
    // SAFETY: interrupt handler contract grants exclusive access to this timer interrupt.
    unsafe fn on_interrupt() {
        // SAFETY: TIMER0 is owned by this interrupt handler and address comes from PAC.
        unsafe {
            let regs = embassy_nrf::pac::timer::Timer::from_ptr(pac::TIMER0.as_ptr());
            regs.events_compare(1).write(|w| *w = 0);
        }
        crate::legacy_common::fodtrigger::FOD_TICK_SIG.signal(());
    }
}
