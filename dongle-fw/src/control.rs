use core::sync::atomic::{AtomicPtr, Ordering};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use protodongers::control::usb_mux::UsbMuxCtrlMsg;
use static_cell::StaticCell;

// Event channel: device -> host control messages (responses, async events)
pub static CONTROL_EVT_CH: StaticCell<Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>> =
    StaticCell::new();
static EVT_PTR: AtomicPtr<Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>> =
    AtomicPtr::new(core::ptr::null_mut());

// Command channel: host -> device control messages (requests)
pub static CONTROL_CMD_CH: StaticCell<Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>> =
    StaticCell::new();
static CMD_PTR: AtomicPtr<Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>> =
    AtomicPtr::new(core::ptr::null_mut());

pub fn init() -> (
    &'static Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>,
    &'static Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>,
) {
    (
        CONTROL_EVT_CH.init(Channel::new()),
        CONTROL_CMD_CH.init(Channel::new()),
    )
}

pub fn register(
    evt_ch: &'static Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>,
    cmd_ch: &'static Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>,
) {
    EVT_PTR.store(evt_ch as *const _ as *mut _, Ordering::Release);
    CMD_PTR.store(cmd_ch as *const _ as *mut _, Ordering::Release);
}

// Non-blocking send of an event to host
pub fn try_send_event(evt: UsbMuxCtrlMsg) {
    let p = EVT_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        let ch: &Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4> = unsafe { &*p };
        let _ = ch.try_send(evt);
    }
}

// Non-blocking receive of a pending event (used by EP0 control_in)
pub fn try_recv_event() -> Option<UsbMuxCtrlMsg> {
    let p = EVT_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        let ch: &Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4> = unsafe { &*p };
        if let Ok(m) = ch.try_receive() {
            return Some(m);
        }
    }
    None
}

#[allow(dead_code)]
// Async receive of events (device -> host)
pub async fn recv_event() -> UsbMuxCtrlMsg {
    loop {
        if let Some(m) = try_recv_event() {
            return m;
        }
        // avoid busy spinning
        embassy_time::Timer::after_millis(1).await;
    }
}

// Non-blocking send of a command from host (used by EP0 control_out)
pub fn try_send_cmd(cmd: UsbMuxCtrlMsg) {
    let p = CMD_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        let ch: &Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4> = unsafe { &*p };
        let _ = ch.try_send(cmd);
    }
}

// Async receive of commands (for the executor task)
pub async fn recv_cmd() -> UsbMuxCtrlMsg {
    // Safety: CMD_PTR initialized in register
    loop {
        let p = CMD_PTR.load(Ordering::Acquire);
        if !p.is_null() {
            let ch: &Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4> = unsafe { &*p };
            if let Ok(m) = ch.try_receive() {
                return m;
            }
        }
        // simple yield to avoid busy-spin
        embassy_time::Timer::after_millis(1).await;
    }
}
