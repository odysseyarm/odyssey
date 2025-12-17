use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, signal::Signal};
use protodongers::control::device::TransportMode;

static MODE: AtomicU8 = AtomicU8::new(TransportMode::Ble as u8);
static MODE_CHANGED: Signal<ThreadModeRawMutex, ()> = Signal::new();

pub fn get() -> TransportMode {
    match MODE.load(Ordering::Relaxed) {
        x if x == TransportMode::Usb as u8 => TransportMode::Usb,
        _ => TransportMode::Ble,
    }
}

pub fn set(mode: TransportMode) {
    MODE.store(mode as u8, Ordering::Relaxed);
    MODE_CHANGED.signal(());
}

pub fn is_ble() -> bool {
    get() == TransportMode::Ble
}

pub fn is_usb() -> bool {
    get() == TransportMode::Usb
}

pub async fn wait_for_change() {
    MODE_CHANGED.wait().await;
}
