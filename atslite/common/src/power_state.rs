use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_nrf::twim::Twim;
use embassy_time::{Delay, Duration, Ticker};
use npm1300_rs::NPM1300;
use nrf_fuel_gauge as fg;

use crate::power;

pub static SOC_PERCENT: AtomicU8 = AtomicU8::new(0);
static VBUS_PRESENT: AtomicBool = AtomicBool::new(false);

/// Update LED state based on current battery and BLE connection status.
/// Call this when BLE connection state changes for immediate LED feedback.
pub async fn update_led_for_ble_state() {
    let vbus_present = VBUS_PRESENT.load(Ordering::Relaxed);
    let soc = SOC_PERCENT.load(Ordering::Relaxed) as f32;
    let ble_connected = crate::pmic_leds::is_ble_connected();

    let led_state = crate::pmic_leds::LedState::from_battery(vbus_present, soc, ble_connected);
    crate::pmic_leds::set_state_seamless_global(led_state).await;
}

#[embassy_executor::task]
pub async fn power_state_task(
    pmic: &'static embassy_sync::mutex::Mutex<
        embassy_sync::blocking_mutex::raw::NoopRawMutex,
        NPM1300<Twim<'static>, Delay>,
    >,
    leds: &'static crate::pmic_leds::PmicLedsHandle,
    battery_model: &'static nrfxlib_sys::battery_model,
) -> ! {
    use embassy_time::Instant;

    let (v0, t0) = {
        let mut guard = pmic.lock().await;
        let v0 = guard.measure_vbat().await.unwrap();
        let t0 = guard.measure_die_temperature().await.unwrap();
        (v0, t0)
    };

    let init = fg::Initial { v0, i0: 0.0, t0 };
    let cfg = fg::default_config();
    let mut gauge = fg::FuelGauge::init(battery_model, init, Some(&cfg)).unwrap();

    let mut tick = Ticker::every(Duration::from_millis(500));
    let mut last = Instant::now();

    pmic.lock()
        .await
        .configure_auto_ibat_measurement(true)
        .await
        .unwrap();

    loop {
        tick.next().await;
        let now = Instant::now();
        let dt = (now - last).as_micros() as f32 / 1_000_000.0;
        last = now;
        defmt::trace!("dt = {}s", dt);

        let (v, i, t) = {
            let mut pm = pmic.lock().await;
            let v = pm.measure_vbat().await.unwrap();
            let i = match pm
                .calculate_ibat(power::DISCHARGE_CURRENT, power::CHARGE_CURRENT_MA)
                .await
            {
                Ok(scale) => scale,
                Err(_) => {
                    defmt::error!("Failed to get full scale current from PMIC");
                    continue;
                }
            };
            defmt::trace!("Measured IBAT: {} µA", i);
            let t = pm.measure_die_temperature().await.unwrap();
            defmt::trace!("Measured die temperature: {} °C", t);
            (v, i as f32 / 1_000_000.0, t)
        };

        let vbus_present = {
            let mut pm = pmic.lock().await;
            pm.get_vbus_in_status()
                .await
                .map(|s| s.is_vbus_in_present)
                .unwrap()
        };

        let soc = gauge.update(v, i, t, dt, vbus_present);

        SOC_PERCENT.store(soc as u8, Ordering::Relaxed);
        VBUS_PRESENT.store(vbus_present, Ordering::Relaxed);

        defmt::trace!("Soc percent: {}%", soc);
        defmt::trace!("VBUS present: {}", vbus_present);

        // Use helper to get the correct LED state based on battery and BLE status
        let ble_connected = crate::pmic_leds::is_ble_connected();
        let led_state = crate::pmic_leds::LedState::from_battery(vbus_present, soc, ble_connected);
        leds.set_state_seamless(led_state).await;
    }
}
