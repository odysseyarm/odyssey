use core::sync::atomic::{AtomicPtr, Ordering};

use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use odyssey_protocol::control::usb_mux::UsbMuxCtrlMsg;
use static_cell::StaticCell;

// Device -> host: responses and async events.
pub static CONTROL_EVT_CH: StaticCell<Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>> =
    StaticCell::new();
static EVT_PTR: AtomicPtr<Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4>> =
    AtomicPtr::new(core::ptr::null_mut());

// Host -> device: requests.
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

/// Queues an event for the host without blocking. Drops it, with a log line,
/// when the channel is full or not yet registered.
pub fn try_send_event(evt: UsbMuxCtrlMsg) {
    let p = EVT_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        let ch: &Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4> = unsafe { &*p };
        match ch.try_send(evt) {
            Ok(()) => defmt::trace!("Control event queued successfully"),
            Err(_) => defmt::warn!("Control event channel full, event dropped!"),
        }
    } else {
        defmt::error!("Control EVT_PTR is null, cannot send event!");
    }
}

/// Takes a pending event without blocking. Used by EP0 control_in.
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

/// Drops every pending event, so stale responses are not read as replies to a
/// new request.
pub fn clear_events() {
    while try_recv_event().is_some() {}
}

/// Queues a host command without blocking. Used by EP0 control_out; drops the
/// command when the channel is full or not yet registered.
pub fn try_send_cmd(cmd: UsbMuxCtrlMsg) {
    let p = CMD_PTR.load(Ordering::Acquire);
    if !p.is_null() {
        let ch: &Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4> = unsafe { &*p };
        match ch.try_send(cmd) {
            Ok(()) => defmt::info!("Control cmd queued successfully"),
            Err(_) => defmt::error!("Control cmd channel full, command dropped!"),
        }
    } else {
        defmt::error!("Control CMD_PTR is null, cannot send command!");
    }
}

/// Waits for the next host command by polling every 1 ms.
pub async fn recv_cmd() -> UsbMuxCtrlMsg {
    loop {
        let p = CMD_PTR.load(Ordering::Acquire);
        if !p.is_null() {
            // SAFETY: a non-null CMD_PTR was stored by `register` from a `&'static` channel.
            let ch: &Channel<ThreadModeRawMutex, UsbMuxCtrlMsg, 4> = unsafe { &*p };
            if let Ok(m) = ch.try_receive() {
                return m;
            }
        }
        embassy_time::Timer::after_millis(1).await;
    }
}
