use embassy_nrf::gpio::Input;
use embassy_time::{Duration, Instant, Timer};

const LONG_PRESS: Duration = Duration::from_secs(1);
const DEBOUNCE_MS: u64 = 30;
const POLL_MS: u64 = 20;

pub async fn power_button_loop<I2c, Delay>(
    mut btn: Input<'_>,
    pmic: &embassy_sync::mutex::Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, npm1300_rs::NPM1300<I2c, Delay>>,
    leds: &crate::pmic_leds::PmicLedsHandle,
) -> !
where
    I2c: embedded_hal_async::i2c::I2c,
    Delay: embedded_hal_async::delay::DelayNs,
{
    let window = Duration::from_millis(1500);
    let mut tap_count: u8 = 0;
    let mut window_start: Option<Instant> = None;

    loop {
        // Reset triple-tap window if expired
        if let Some(ws) = window_start {
            if Instant::now() - ws > window {
                tap_count = 0;
                window_start = None;
            }
        }

        // Wait for button press
        btn.wait_for_low().await;
        Timer::after_millis(DEBOUNCE_MS).await;
        if btn.is_high() {
            continue;
        }

        // Measure hold duration
        let t0 = Instant::now();
        loop {
            if btn.is_high() {
                break;
            }
            if Instant::now() - t0 >= LONG_PRESS {
                break;
            }
            Timer::after_millis(POLL_MS).await;
        }
        let held = Instant::now() - t0;

        if held >= LONG_PRESS {
            // Long press -> power off
            leds.lock_and_set_state(crate::pmic_leds::LedState::TurningOff).await;
            // ensure release before powering off
            btn.wait_for_high().await;
            let _ = crate::power::handle_long_press_power_off(pmic).await;
            loop {}
        } else {
            // Short tap detected
            while btn.is_low() {
                Timer::after_millis(10).await;
            }
            Timer::after_millis(DEBOUNCE_MS).await;

            if window_start.is_none() {
                window_start = Some(Instant::now());
                tap_count = 1;
            } else {
                tap_count = tap_count.saturating_add(1);
            }

            if tap_count >= 3 {
                defmt::info!("Triple press detected");
                // Enter pairing mode for 120s
                crate::ble::peripheral::enter_pairing_mode();
                defmt::info!("[pair] entered pairing mode: enabling MSD in scan response");
                // Clear any existing BLE bond so pairing is fresh
                unsafe { crate::settings::get_settings() }.ble_bond_clear();
                crate::ble::peripheral::request_disconnect();
                leds.lock_and_set_state(crate::pmic_leds::LedState::Pairing).await;
                let timeout = Duration::from_secs(120);
                let deadline = Instant::now() + timeout;
                // While in pairing, still allow long-press power-off
                loop {
                    if !crate::ble::peripheral::is_pairing_active() {
                        break;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    // Check for long press without blocking
                    if btn.is_low() {
                        let t0 = Instant::now();
                        while btn.is_low() {
                            if Instant::now() - t0 >= LONG_PRESS {
                                // Power off during pairing
                                leds.lock_and_set_state(crate::pmic_leds::LedState::TurningOff).await;
                                btn.wait_for_high().await;
                                let _ = crate::power::handle_long_press_power_off(pmic).await;
                                loop {}
                            }
                            Timer::after_millis(POLL_MS).await;
                        }
                    }
                    Timer::after_millis(100).await;
                }
                crate::ble::peripheral::cancel_pairing_mode();
                // Clear any pending disconnect request since pairing is complete
                // and we want to keep the new connection
                let _ = crate::ble::peripheral::take_disconnect_request();
                leds.unlock_state().await;

                // Reset tap window
                tap_count = 0;
                window_start = None;
            }
        }
    }
}

pub async fn wait_for_power_button_full_press(btn: &mut Input<'_>, hold_time: Duration) {
    loop {
        defmt::trace!("Waiting for power button down...");
        btn.wait_for_low().await;
        Timer::after_millis(DEBOUNCE_MS).await;
        if btn.is_high() {
            continue;
        }

        let t0 = Instant::now();
        loop {
            if btn.is_high() {
                defmt::trace!("Power button released before long-press threshold");
                break;
            }
            if Instant::now().duration_since(t0) >= hold_time {
                defmt::trace!("Power button long-press detected");
                return;
            }
            Timer::after_millis(POLL_MS).await;
        }
    }
}
