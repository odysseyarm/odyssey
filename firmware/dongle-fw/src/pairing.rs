use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, Timer};
use odyssey_protocol::control::usb_mux::{PairingError, PairingOutcome, UsbMuxCtrlMsg};

static PAIRING: AtomicBool = AtomicBool::new(false);
static DEADLINE: Mutex<CriticalSectionRawMutex, RefCell<Option<Instant>>> =
    Mutex::new(RefCell::new(None));

pub fn is_active() -> bool {
    PAIRING.load(Ordering::Relaxed)
}

pub fn enter() {
    DEADLINE.lock(|deadline| *deadline.borrow_mut() = None);
    PAIRING.store(true, Ordering::Relaxed);
    crate::ble::central::reset_pairing_capture();
    defmt::info!("Signaling RESTART_CENTRAL from pairing::enter()");
    crate::ble::central::RESTART_CENTRAL.signal(());
}

/// Ends pairing and removes its deadline so a cancelled session cannot later time out.
pub fn cancel() {
    PAIRING.store(false, Ordering::Relaxed);
    DEADLINE.lock(|deadline| *deadline.borrow_mut() = None);
}

pub fn enter_with_timeout(timeout: Duration) {
    enter();
    DEADLINE.lock(|deadline| *deadline.borrow_mut() = Some(Instant::now() + timeout));
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
            let expired =
                DEADLINE.lock(|deadline| deadline.borrow().is_some_and(|dl| Instant::now() >= dl));
            if is_active() && expired {
                cancel();
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
