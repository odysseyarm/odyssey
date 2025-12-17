#![allow(static_mut_refs)]
// Real L2CAP implementation for ATSlite (wraps BLE stack)

use common::protodongers::Packet;
use core::future::Future;
use core::sync::atomic::{AtomicBool, Ordering};

// Re-export the BLE L2CAP types
pub use crate::ble::l2cap_bridge::{get_or_init, L2capChannels as Inner};
use crate::ble::l2cap_bridge::{
    L2capControlRxChannel, L2capControlTxChannel, L2capDataRxChannel, L2capDataTxChannel,
};

pub struct AtsliteL2capChannels {
    inner: Inner,
}

impl AtsliteL2capChannels {
    pub fn new() -> Self {
        Self {
            inner: get_or_init(),
        }
    }
}

// Wrapper types that preserve the actual channel type
pub enum L2capRxWrapper {
    Control(&'static L2capControlRxChannel),
    Data(&'static L2capDataRxChannel),
}

pub enum L2capTxWrapper {
    Control(&'static L2capControlTxChannel),
    Data(&'static L2capDataTxChannel),
}

// Wrappers that must live for 'static to satisfy the trait signatures.
// We guard initialization ourselves to avoid double-init panics when reconnecting.
static WRAPPERS_INIT: AtomicBool = AtomicBool::new(false);
static mut CONTROL_RX_WRAPPER: Option<L2capRxWrapper> = None;
static mut DATA_RX_WRAPPER: Option<L2capRxWrapper> = None;
static mut CONTROL_TX_WRAPPER: Option<L2capTxWrapper> = None;
static mut DATA_TX_WRAPPER: Option<L2capTxWrapper> = None;

fn ensure_wrappers(inner: &Inner) {
    if WRAPPERS_INIT.load(Ordering::Acquire) {
        return;
    }

    if WRAPPERS_INIT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        unsafe {
            CONTROL_RX_WRAPPER = Some(L2capRxWrapper::Control(inner.control_rx));
            DATA_RX_WRAPPER = Some(L2capRxWrapper::Data(inner.data_rx));
            CONTROL_TX_WRAPPER = Some(L2capTxWrapper::Control(inner.control_tx));
            DATA_TX_WRAPPER = Some(L2capTxWrapper::Data(inner.data_tx));
        }
    } else {
        // Another thread/task is initializing; wait until it's done.
        while !WRAPPERS_INIT.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
    }
}

// Implement the L2capChannels trait
impl common::platform::L2capChannels for AtsliteL2capChannels {
    type Receiver = L2capRxWrapper;
    type Sender = L2capTxWrapper;

    fn control_rx(&self) -> &Self::Receiver {
        ensure_wrappers(&self.inner);
        unsafe { CONTROL_RX_WRAPPER.as_ref().unwrap() }
    }

    fn data_rx(&self) -> &Self::Receiver {
        ensure_wrappers(&self.inner);
        unsafe { DATA_RX_WRAPPER.as_ref().unwrap() }
    }

    fn control_tx(&self) -> &Self::Sender {
        ensure_wrappers(&self.inner);
        unsafe { CONTROL_TX_WRAPPER.as_ref().unwrap() }
    }

    fn data_tx(&self) -> &Self::Sender {
        ensure_wrappers(&self.inner);
        unsafe { DATA_TX_WRAPPER.as_ref().unwrap() }
    }
}

impl common::platform::L2capReceiver for L2capRxWrapper {
    fn receive(&self) -> impl Future<Output = Packet> {
        async move {
            match self {
                L2capRxWrapper::Control(ch) => ch.receive().await,
                L2capRxWrapper::Data(ch) => ch.receive().await,
            }
        }
    }
}

impl common::platform::L2capSender for L2capTxWrapper {
    fn send(&self, pkt: Packet) -> impl Future<Output = ()> {
        async move {
            match self {
                L2capTxWrapper::Control(ch) => ch.send(pkt).await,
                L2capTxWrapper::Data(ch) => ch.send(pkt).await,
            }
        }
    }

    fn try_send(&self, pkt: Packet) -> Result<(), ()> {
        match self {
            L2capTxWrapper::Control(ch) => ch.try_send(pkt).map_err(|_| ()),
            L2capTxWrapper::Data(ch) => ch.try_send(pkt).map_err(|_| ()),
        }
    }
}
