//! Device control channels behind the USB vendor requests: pairing, bond
//! management and version queries on lite/vm devices.

use core::sync::atomic::{AtomicPtr, Ordering};

use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use odyssey_protocol::control::device::DeviceMsg;
use static_cell::StaticCell;

// Device -> host: responses and async events.
/// Since: Odyssey Desktop 0.7.0.
pub static CONTROL_EVT_CH: StaticCell<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    StaticCell::new();
static EVT_PTR: AtomicPtr<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    AtomicPtr::new(core::ptr::null_mut());

// Host -> device: requests.
/// Since: Odyssey Desktop 0.7.0.
pub static CONTROL_CMD_CH: StaticCell<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    StaticCell::new();
static CMD_PTR: AtomicPtr<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    AtomicPtr::new(core::ptr::null_mut());

/// Since: Odyssey Desktop 0.7.0.
pub fn init() -> (
    &'static Channel<ThreadModeRawMutex, DeviceMsg, 8>,
    &'static Channel<ThreadModeRawMutex, DeviceMsg, 8>,
) {
    (
        CONTROL_EVT_CH.init(Channel::new()),
        CONTROL_CMD_CH.init(Channel::new()),
    )
}

/// Since: Odyssey Desktop 0.7.0.
pub fn register(
    evt_ch: &'static Channel<ThreadModeRawMutex, DeviceMsg, 8>,
    cmd_ch: &'static Channel<ThreadModeRawMutex, DeviceMsg, 8>,
) {
    EVT_PTR.store(evt_ch as *const _ as *mut _, Ordering::Release);
    CMD_PTR.store(cmd_ch as *const _ as *mut _, Ordering::Release);
}

/// Queues an event for the host. Dropped when the channel is unregistered or full.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn try_send_event(evt: DeviceMsg) {
    let p = EVT_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: register stores a live `'static` channel pointer before publication.
        let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
        let _ = ch.try_send(evt);
    }
}

/// Pending event for EP0 control_in, if any.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn try_recv_event() -> Option<DeviceMsg> {
    let p = EVT_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: register stores a live `'static` channel pointer before publication.
        let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
        if let Ok(m) = ch.try_receive() {
            return Some(m);
        }
    }
    None
}

/// Queues a host command from EP0 control_out without blocking.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn try_send_cmd(cmd: DeviceMsg) {
    let p = CMD_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: register stores a live `'static` channel pointer before publication.
        let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
        let _ = ch.try_send(cmd);
    }
}

/// Waits for the next host command. Used by the executor task.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn recv_cmd() -> DeviceMsg {
    loop {
        let p = CMD_PTR.load(Ordering::Acquire);
        if !p.is_null() {
            // SAFETY: register stores a live `'static` channel pointer before publication.
            let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
            if let Ok(m) = ch.try_receive() {
                return m;
            }
        }
        embassy_time::Timer::after_millis(1).await;
    }
}
