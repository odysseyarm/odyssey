use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};

static PAIRING: AtomicBool = AtomicBool::new(false);
static PAIRING_DONE: Signal<ThreadModeRawMutex, ()> = Signal::new();
static SCAN_RESTART: Signal<ThreadModeRawMutex, ()> = Signal::new();
static SCAN_RESTART_COUNTER: AtomicU32 = AtomicU32::new(0);
static mut DEADLINE: Option<Instant> = None;
static TIMEOUT_HIT: AtomicBool = AtomicBool::new(false);

pub fn is_active() -> bool {
    PAIRING.load(Ordering::Relaxed)
}

pub fn enter() {
    PAIRING.store(true, Ordering::Relaxed);
    // Wake any waiters
    PAIRING_DONE.signal(());
    // Signal scan to restart
    SCAN_RESTART_COUNTER.fetch_add(1, Ordering::Relaxed);
    SCAN_RESTART.signal(());
}

pub fn cancel() {
    PAIRING.store(false, Ordering::Relaxed);
    // Signal scan to restart
    SCAN_RESTART_COUNTER.fetch_add(1, Ordering::Relaxed);
    SCAN_RESTART.signal(());
}

pub fn enter_with_timeout(timeout: Duration) {
    enter();
    unsafe {
        DEADLINE = Some(Instant::now() + timeout);
    }
}

pub fn take_timeout() -> bool {
    TIMEOUT_HIT.swap(false, Ordering::AcqRel)
}

#[allow(dead_code)]
pub fn get_restart_counter() -> u32 {
    SCAN_RESTART_COUNTER.load(Ordering::Relaxed)
}

#[allow(dead_code)]
pub async fn wait_for_scan_restart() {
    SCAN_RESTART.wait().await
}

#[embassy_executor::task]
pub async fn pairing_led_task(mut blue: embassy_nrf::gpio::Output<'static>) -> ! {
    loop {
        if is_active() {
            // Flash blue while pairing
            blue.set_low();
            Timer::after(Duration::from_millis(200)).await;
            blue.set_high();
            Timer::after(Duration::from_millis(200)).await;
            // Check timeout
            unsafe {
                if let Some(dl) = DEADLINE {
                    if Instant::now() >= dl {
                        cancel();
                        TIMEOUT_HIT.store(true, Ordering::Release);
                        DEADLINE = None;
                    }
                }
            }
        } else {
            // Idle: ensure LED off
            blue.set_high();
            Timer::after(Duration::from_millis(500)).await;
        }
    }
}
