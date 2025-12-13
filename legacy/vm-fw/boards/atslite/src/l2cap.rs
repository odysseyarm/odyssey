// Real L2CAP implementation for ATSlite (wraps BLE stack)

use common::protodongers::Packet;
use core::future::Future;

// Re-export the BLE L2CAP types
pub use crate::ble::l2cap_bridge::{L2capChannels as Inner, get_or_init};
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

// Implement the L2capChannels trait
impl common::platform::L2capChannels for AtsliteL2capChannels {
    type Receiver = L2capRxWrapper;
    type Sender = L2capTxWrapper;

    fn control_rx(&self) -> &Self::Receiver {
        // SAFETY: This is safe because L2capRxWrapper is a transparent wrapper
        unsafe {
            &*(self.inner.control_rx as *const L2capControlRxChannel as *const L2capRxWrapper)
        }
    }

    fn data_rx(&self) -> &Self::Receiver {
        unsafe { &*(self.inner.data_rx as *const L2capDataRxChannel as *const L2capRxWrapper) }
    }

    fn control_tx(&self) -> &Self::Sender {
        unsafe {
            &*(self.inner.control_tx as *const L2capControlTxChannel as *const L2capTxWrapper)
        }
    }

    fn data_tx(&self) -> &Self::Sender {
        unsafe { &*(self.inner.data_tx as *const L2capDataTxChannel as *const L2capTxWrapper) }
    }
}

#[repr(transparent)]
pub struct L2capRxWrapper(());

#[repr(transparent)]
pub struct L2capTxWrapper(());

impl common::platform::L2capReceiver for L2capRxWrapper {
    fn receive(&self) -> impl Future<Output = Packet> {
        // Forward to the actual channel receive
        async move {
            // Cast back to the real type
            let channel = unsafe { &*(self as *const _ as *const L2capControlRxChannel) };
            channel.receive().await
        }
    }
}

impl common::platform::L2capSender for L2capTxWrapper {
    fn send(&self, pkt: Packet) -> impl Future<Output = ()> {
        async move {
            let channel = unsafe { &*(self as *const _ as *const L2capControlTxChannel) };
            channel.send(pkt).await
        }
    }

    fn try_send(&self, pkt: Packet) -> Result<(), ()> {
        let channel = unsafe { &*(self as *const _ as *const L2capControlTxChannel) };
        channel.try_send(pkt).map_err(|_| ())
    }
}
