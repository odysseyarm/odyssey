use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use backon::{ConstantBuilder, EmbassySleeper, Retryable};
use embassy_nrf::mode::Async;
use embassy_nrf::twim::Twim;
use embassy_time::{Delay, Duration, Ticker};
use npm1300_rs::NPM1300;
use nrf_fuel_gauge as fg;

use crate::power;

/// Since: Odyssey Desktop 0.7.0.
pub static SOC_PERCENT: AtomicU8 = AtomicU8::new(0);
/// Since: Odyssey Desktop 0.7.0.
pub static VBUS_PRESENT: AtomicBool = AtomicBool::new(false);

/// Re-derives the LED state from the last battery reading and the BLE connection. Call it when
/// the BLE connection changes so the LED reacts without waiting for the next battery tick.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn update_led_for_ble_state() {
    let vbus_present = VBUS_PRESENT.load(Ordering::Relaxed);
    let soc = SOC_PERCENT.load(Ordering::Relaxed) as f32;
    let ble_connected = crate::pmic_leds::is_ble_connected();

    let led_state = crate::pmic_leds::LedState::from_battery(vbus_present, soc, ble_connected);
    crate::pmic_leds::set_state_seamless_global(led_state).await;
}

/// A failed or skipped sample must not advance the caller's `last_us`, so `dt` keeps covering it.
fn elapsed_secs(last_us: u64, now_us: u64) -> f32 {
    now_us.saturating_sub(last_us) as f32 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::elapsed_secs;

    #[test]
    fn skipped_samples_accumulate_until_success_resets_clock() {
        let mut last_us = 500_000;
        // Ticks at 1.0 s and 1.5 s were skipped; the next success covers 1.5 s.
        assert_eq!(elapsed_secs(last_us, 2_000_000), 1.5);
        last_us = 2_000_000;
        assert_eq!(elapsed_secs(last_us, 2_500_000), 0.5);
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[embassy_executor::task]
pub async fn power_state_task(
    pmic: &'static embassy_sync::mutex::Mutex<
        embassy_sync::blocking_mutex::raw::NoopRawMutex,
        NPM1300<Twim<'static, Async>, Delay>,
    >,
    leds: &'static crate::pmic_leds::PmicLedsHandle,
    battery_model: &'static nrfxlib_sys::battery_model,
) -> ! {
    use embassy_time::Instant;

    // The ADC reports an invalid battery voltage until its first conversion completes.
    // `None` means the read itself failed (already logged); no measurement exists.
    struct AdcNotReady(Option<f32>);
    let (v0, t0) = (|| async {
        let mut guard = pmic.lock().await;
        let v = guard.measure_vbat().await.map_err(|error| {
            defmt::error!(
                "[fg] init vbat read failed: {:?}",
                defmt::Debug2Format(&error)
            );
            AdcNotReady(None)
        })?;
        if v > 2.5 {
            let t = guard.measure_die_temperature().await.map_err(|error| {
                defmt::error!(
                    "[fg] init temperature read failed: {:?}",
                    defmt::Debug2Format(&error)
                );
                AdcNotReady(None)
            })?;
            Ok((v, t))
        } else {
            Err(AdcNotReady(Some(v)))
        }
    })
    .retry(
        ConstantBuilder::default()
            .with_delay(core::time::Duration::from_millis(50))
            .without_max_times(),
    )
    .sleep(EmbassySleeper)
    .notify(|e: &AdcNotReady, _| match e.0 {
        Some(v) => defmt::trace!("[fg] vbat={} V invalid, waiting for ADC...", v),
        None => defmt::trace!("[fg] init PMIC read failed, retrying..."),
    })
    .await
    .unwrap_or_else(|_| defmt::unreachable!("unbounded retry gave up during init"));

    defmt::trace!("[fg] init: v0={} V, t0={} °C", v0, t0);

    let init = fg::Initial { v0, i0: 0.0, t0 };
    let cfg = fg::default_config().unwrap();
    let mut gauge = fg::FuelGauge::init(battery_model, init, Some(&cfg)).unwrap();

    let mut tick = Ticker::every(Duration::from_millis(500));
    let mut last_us = Instant::now().as_micros();

    (|| async {
        pmic.lock()
            .await
            .configure_auto_ibat_measurement(true)
            .await
            .inspect_err(|error| {
                defmt::error!(
                    "[fg] auto ibat config failed: {:?}",
                    defmt::Debug2Format(error)
                );
            })
    })
    .retry(
        ConstantBuilder::default()
            .with_delay(core::time::Duration::from_millis(50))
            .without_max_times(),
    )
    .sleep(EmbassySleeper)
    .await
    .unwrap_or_else(|_| defmt::unreachable!("unbounded retry gave up configuring ibat"));

    loop {
        tick.next().await;
        let now = Instant::now();
        let dt = elapsed_secs(last_us, now.as_micros());

        let (v, i, t) = {
            let mut pm = pmic.lock().await;
            let v = match pm.measure_vbat().await {
                Ok(v) => v,
                Err(error) => {
                    defmt::error!("[fg] vbat read failed: {:?}", defmt::Debug2Format(&error));
                    continue;
                }
            };
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
            let t = match pm.measure_die_temperature().await {
                Ok(t) => t,
                Err(error) => {
                    defmt::error!(
                        "[fg] temperature read failed: {:?}",
                        defmt::Debug2Format(&error)
                    );
                    continue;
                }
            };
            defmt::trace!("[fg] v={} V  ibat={} µA  t={} °C  dt={} s", v, i, t, dt);
            (v, i as f32 / 1_000_000.0, t)
        };

        let vbus_present = {
            let mut pm = pmic.lock().await;
            match pm.get_vbus_in_status().await {
                Ok(s) => s.is_vbus_in_present,
                Err(error) => {
                    defmt::error!("[fg] VBUS read failed: {:?}", defmt::Debug2Format(&error));
                    continue;
                }
            }
        };

        let soc = match gauge.update(v, i, t, dt, vbus_present) {
            Ok(soc) => soc,
            Err(error) => {
                defmt::error!("[fg] update failed: {:?}", defmt::Debug2Format(&error));
                continue;
            }
        };
        // Only a processed sample consumes the interval; skipped ticks accumulate into the next dt.
        last_us = now.as_micros();

        SOC_PERCENT.store(soc as u8, Ordering::Relaxed);
        VBUS_PRESENT.store(vbus_present, Ordering::Relaxed);

        defmt::trace!("[fg] soc={} %  vbus={}", soc, vbus_present);

        let ble_connected = crate::pmic_leds::is_ble_connected();
        let led_state = crate::pmic_leds::LedState::from_battery(vbus_present, soc, ble_connected);
        leds.set_state_seamless(led_state).await;
    }
}
