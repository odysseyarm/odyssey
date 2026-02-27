//! Device control channel for USB vendor requests
//! Used for pairing, bond management, and version queries on lite/vm devices

use core::sync::atomic::{AtomicPtr, Ordering};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use protodongers::control::device::DeviceMsg;
use static_cell::StaticCell;

// Event channel: device -> host control messages (responses, async events)
pub static CONTROL_EVT_CH: StaticCell<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    StaticCell::new();
static EVT_PTR: AtomicPtr<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    AtomicPtr::new(core::ptr::null_mut());

// Command channel: host -> device control messages (requests)
pub static CONTROL_CMD_CH: StaticCell<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    StaticCell::new();
static CMD_PTR: AtomicPtr<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    AtomicPtr::new(core::ptr::null_mut());

pub fn init() -> (
    &'static Channel<ThreadModeRawMutex, DeviceMsg, 8>,
    &'static Channel<ThreadModeRawMutex, DeviceMsg, 8>,
) {
    (
        CONTROL_EVT_CH.init(Channel::new()),
        CONTROL_CMD_CH.init(Channel::new()),
    )
}

pub fn register(
    evt_ch: &'static Channel<ThreadModeRawMutex, DeviceMsg, 8>,
    cmd_ch: &'static Channel<ThreadModeRawMutex, DeviceMsg, 8>,
) {
    EVT_PTR.store(evt_ch as *const _ as *mut _, Ordering::Release);
    CMD_PTR.store(cmd_ch as *const _ as *mut _, Ordering::Release);
}

// Non-blocking send of an event to host
pub fn try_send_event(evt: DeviceMsg) {
    let p = EVT_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
        let _ = ch.try_send(evt);
    }
}

// Non-blocking receive of a pending event (used by EP0 control_in)
pub fn try_recv_event() -> Option<DeviceMsg> {
    let p = EVT_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
        if let Ok(m) = ch.try_receive() {
            return Some(m);
        }
    }
    None
}

// Non-blocking send of a command from host (used by EP0 control_out)
pub fn try_send_cmd(cmd: DeviceMsg) {
    let p = CMD_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
        let _ = ch.try_send(cmd);
    }
}

// Async receive of commands (for the executor task)
pub async fn recv_cmd() -> DeviceMsg {
    loop {
        let p = CMD_PTR.load(Ordering::Acquire);
        if !p.is_null() {
            let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
            if let Ok(m) = ch.try_receive() {
                return m;
            }
        }
        embassy_time::Timer::after_millis(1).await;
    }
}
