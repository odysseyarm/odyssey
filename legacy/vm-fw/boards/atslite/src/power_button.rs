use embassy_nrf::gpio::Input;
use embassy_time::{Duration, Instant, Timer};

const LONG_PRESS: Duration = Duration::from_secs(1);
const DEBOUNCE_MS: u64 = 30;
const POLL_MS: u64 = 20;

pub async fn power_button_loop<I2c, Delay>(
    mut btn: Input<'_>,
    pmic: &embassy_sync::mutex::Mutex<
        embassy_sync::blocking_mutex::raw::NoopRawMutex,
        npm1300_rs::NPM1300<I2c, Delay>,
    >,
    leds: &crate::pmic_leds::PmicLedsHandle,
) -> !
where
    I2c: embedded_hal_async::i2c::I2c,
    Delay: embedded_hal_async::delay::DelayNs,
{
    loop {
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
            defmt::info!("Long press detected - powering off");
            leds.lock_and_set_state(crate::pmic_leds::LedState::TurningOff)
                .await;
            // Ensure release before powering off
            btn.wait_for_high().await;
            let _ = crate::power::handle_long_press_power_off(pmic).await;
            loop {}
        }
        // Short presses are ignored - use CLI for pairing/bond management
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
