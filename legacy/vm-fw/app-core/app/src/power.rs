use embassy_nrf::pac;
use embassy_sync::{{blocking_mutex::raw::CriticalSectionRawMutex}, signal::Signal};
use npm1300_rs::charger::DischargeCurrentLimit;
use npm1300_rs::sysreg::VbusInCurrentLimit;

pub static VBUS_REMOVED_SIG: Signal<CriticalSectionRawMutex, ()> = Signal::new();
const GPREGRET_PENDING_SHIP: u8 = 0x42;
pub const CHARGE_CURRENT_MA: u16 = 310;
pub const DISCHARGE_CURRENT: DischargeCurrentLimit = DischargeCurrentLimit::Low;

pub fn notify_vbus_removed() {
    VBUS_REMOVED_SIG.signal(());
}

pub fn set_pending_ship_flag() {
    let p = pac::POWER;
    p.gpregret(0).write(|w| w.set_gpregret(GPREGRET_PENDING_SHIP));
}

pub fn take_pending_ship_flag() -> bool {
    let p = pac::POWER;
    let v = p.gpregret(0).read().gpregret();
    p.gpregret(0).write(|w| w.set_gpregret(0));
    v == GPREGRET_PENDING_SHIP
}

pub async fn handle_long_press_power_off<I2c, Delay>(
    pmic: &embassy_sync::mutex::Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, npm1300_rs::NPM1300<I2c, Delay>>,
) -> Result<!, npm1300_rs::NPM1300Error<I2c::Error>>
where
    I2c: embedded_hal_async::i2c::I2c,
    Delay: embedded_hal_async::delay::DelayNs,
{
    use cortex_m::peripheral::SCB;

    let vbus_present = {
        let mut guard = pmic.lock().await;
        guard.get_vbus_in_status().await?.is_vbus_in_present
    };

    if !vbus_present {
        defmt::trace!("Enter ship mode");
        pmic.lock().await.enter_ship_mode().await?;
        loop {
            cortex_m::asm::wfe();
        }
    }

    crate::power::set_pending_ship_flag();

    defmt::trace!("System resetting into faux off due to long-press with VBUS present");

    SCB::sys_reset();
}

pub async fn pmic_setup<I2c, Delay>(
    pmic: &mut npm1300_rs::NPM1300<I2c, Delay>,
) -> Result<(), npm1300_rs::NPM1300Error<I2c::Error>>
where
    I2c: embedded_hal_async::i2c::I2c,
    Delay: embedded_hal_async::delay::DelayNs,
{
    use npm1300_rs::{gpios::{GpioMode, GpioConfigBuilder}, mainreg::Vbusin0EventMask, Shphldtim};
    pmic.set_ship_hold_press_timer(Shphldtim::Ms304).await?;
    pmic.use_ship_hold_button_only().await?;
    pmic.configure_gpio(0, GpioConfigBuilder::default().mode(GpioMode::GpoIrq).build()).await?;
    pmic.enable_vbusin0_interrupts(Vbusin0EventMask::VBUS_REMOVED).await?;

    Ok(())
}

pub async fn configure_and_start_charging<I2c, Delay>(
    pmic: &mut npm1300_rs::NPM1300<I2c, Delay>,
    vbus_limit: VbusInCurrentLimit, // e.g. VbusInCurrentLimit::I500mA
) -> Result<(), npm1300_rs::NPM1300Error<I2c::Error>>
where
    I2c: embedded_hal_async::i2c::I2c,
    Delay: embedded_hal_async::delay::DelayNs,
{
    pmic.set_charger_current(CHARGE_CURRENT_MA).await?;
    pmic.clear_charger_errors().await?;
    pmic.set_vbus_in_current_limit(vbus_limit).await?;
    pmic.set_discharge_current_limit(DISCHARGE_CURRENT).await?;
    pmic.set_normal_temperature_termination_voltage(npm1300_rs::charger::ChargerTerminationVoltage::V4_15).await?;
    pmic.set_warm_temperature_termination_voltage(npm1300_rs::charger::ChargerTerminationVoltage::V4_00).await?;
    pmic.use_ntc_measurements().await?;
    pmic.enable_battery_recharge().await?;
    pmic.enable_battery_charging().await
}
