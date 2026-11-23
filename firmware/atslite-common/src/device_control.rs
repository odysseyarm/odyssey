//! Device control messages carried over USB vendor requests and the BLE L2CAP control channel:
//! pairing, bond management and version queries on lite/vm devices.

use core::sync::atomic::{AtomicPtr, Ordering};

use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::channel::Channel;
use odyssey_protocol::control::device::DeviceMsg;
use static_cell::StaticCell;

/// Device -> host messages: responses and async events.
///
/// Since: Odyssey Desktop 0.7.0.
pub static CONTROL_EVT_CH: StaticCell<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    StaticCell::new();
static EVT_PTR: AtomicPtr<Channel<ThreadModeRawMutex, DeviceMsg, 8>> =
    AtomicPtr::new(core::ptr::null_mut());

/// Host -> device requests.
///
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

/// Queues an event for the host. Drops it if the channel is full or not registered yet.
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

/// Pops a pending event. EP0 `control_in` calls this.
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

/// Queues a host command. EP0 `control_out` calls this. Drops it if the channel is full.
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

/// Waits for the next host command, or for registration during startup.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn recv_cmd() -> DeviceMsg {
    loop {
        let p = CMD_PTR.load(Ordering::Acquire);
        if !p.is_null() {
            // SAFETY: register stores a live `'static` channel pointer before publication.
            let ch: &Channel<ThreadModeRawMutex, DeviceMsg, 8> = unsafe { &*p };
            return ch.receive().await;
        }
        embassy_time::Timer::after_millis(1).await;
    }
}

/// Where a command came from, and so where its replies go.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug)]
pub enum Origin {
    /// Replies go to the USB event queue.
    Usb,
    /// Replies go to the L2CAP control channel with the request's packet id.
    Ble {
        /// Packet id of the request.
        id: u8,
        /// BLE session the request arrived in. `None` delivers to whichever session is up, for
        /// results that outlive the link that asked, such as a pairing that drops it.
        session: Option<u32>,
    },
}

impl Origin {
    /// The same origin, delivered to whichever BLE session is up.
    pub(crate) fn detached(self) -> Self {
        match self {
            Self::Ble { id, .. } => Self::Ble { id, session: None },
            Self::Usb => Self::Usb,
        }
    }
}

static BLE_CMD_CH: Channel<CriticalSectionRawMutex, (DeviceMsg, Origin), 4> = Channel::new();

/// Queues a command that arrived over BLE. Drops it if the queue is full.
///
/// Since: Unreleased.
pub fn try_send_ble_cmd(cmd: DeviceMsg, id: u8, session: u32) {
    let origin = Origin::Ble {
        id,
        session: Some(session),
    };
    let _ = BLE_CMD_CH.try_send((cmd, origin));
}

/// Waits for the next command from either transport.
///
/// Since: Unreleased.
pub async fn recv_any_cmd() -> (DeviceMsg, Origin) {
    match embassy_futures::select::select(recv_cmd(), BLE_CMD_CH.receive()).await {
        embassy_futures::select::Either::First(cmd) => (cmd, Origin::Usb),
        embassy_futures::select::Either::Second(cmd) => cmd,
    }
}
