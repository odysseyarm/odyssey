use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use odyssey_protocol::control::usb_mux::{PairingError, PairingOutcome, UsbMuxCtrlMsg};

static PAIRING: AtomicBool = AtomicBool::new(false);
static DEADLINE: Mutex<CriticalSectionRawMutex, Option<Instant>> = Mutex::new(None);

pub fn is_active() -> bool {
    PAIRING.load(Ordering::Relaxed)
}

pub fn enter() {
    PAIRING.store(true, Ordering::Relaxed);
    crate::ble::central::reset_pairing_capture();
    defmt::info!("Signaling RESTART_CENTRAL from pairing::enter()");
    crate::ble::central::RESTART_CENTRAL.signal(());
}

pub fn cancel() {
    PAIRING.store(false, Ordering::Relaxed);
}

pub fn enter_with_timeout(timeout: Duration) {
    enter();
    if let Ok(mut guard) = DEADLINE.try_lock() {
        *guard = Some(Instant::now() + timeout);
    }
}

/// Blinks the pairing LED while pairing is active and ends pairing at its
/// deadline. The timeout goes straight into the EP0 control event queue, so
/// the host reads it on its next control poll whether or not the bulk
/// endpoints are configured.
#[embassy_executor::task]
pub async fn pairing_led_task(mut blue: embassy_nrf::gpio::Output<'static>) -> ! {
    loop {
        if is_active() {
            // The LED is active-low.
            blue.set_low();
            Timer::after(Duration::from_millis(200)).await;
            blue.set_high();
            Timer::after(Duration::from_millis(200)).await;
            if let Ok(mut deadline) = DEADLINE.try_lock()
                && let Some(dl) = *deadline
                && Instant::now() >= dl
            {
                cancel();
                *deadline = None;
                crate::control::try_send_event(UsbMuxCtrlMsg::PairingResult(PairingOutcome::Err(
                    PairingError::Timeout,
                )));
            }
        } else {
            blue.set_high();
            Timer::after(Duration::from_millis(500)).await;
        }
    }
}
