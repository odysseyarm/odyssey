use core::future::Future;

use legacy_common::odyssey_protocol::Packet;

use crate::ble::l2cap_bridge::{
    L2capControlRxChannel, L2capControlTxChannel, L2capDataRxChannel, L2capDataTxChannel, channels,
};

pub struct AtsliteL2capChannels {
    control_rx: L2capRxWrapper,
    data_rx: L2capRxWrapper,
    control_tx: L2capTxWrapper,
    data_tx: L2capTxWrapper,
}

impl AtsliteL2capChannels {
    pub fn new() -> Self {
        let inner = channels();
        Self {
            control_rx: L2capRxWrapper::Control(inner.control_rx),
            data_rx: L2capRxWrapper::Data(inner.data_rx),
            control_tx: L2capTxWrapper::Control(inner.control_tx),
            data_tx: L2capTxWrapper::Data(inner.data_tx),
        }
    }
}

// Keep the concrete channel type so control and data channels stay distinct.
pub enum L2capRxWrapper {
    Control(&'static L2capControlRxChannel),
    Data(&'static L2capDataRxChannel),
}

pub enum L2capTxWrapper {
    Control(&'static L2capControlTxChannel),
    Data(&'static L2capDataTxChannel),
}

impl legacy_common::platform::L2capChannels for AtsliteL2capChannels {
    type Receiver = L2capRxWrapper;
    type Sender = L2capTxWrapper;

    fn control_rx(&self) -> &Self::Receiver {
        &self.control_rx
    }

    fn data_rx(&self) -> &Self::Receiver {
        &self.data_rx
    }

    fn control_tx(&self) -> &Self::Sender {
        &self.control_tx
    }

    fn data_tx(&self) -> &Self::Sender {
        &self.data_tx
    }

    fn session_generation(&self) -> u32 {
        crate::ble::l2cap_bridge::session_generation()
    }
}

impl legacy_common::platform::L2capReceiver for L2capRxWrapper {
    fn receive(&self) -> impl Future<Output = Packet> {
        async move {
            match self {
                L2capRxWrapper::Control(ch) => ch.receive().await,
                L2capRxWrapper::Data(ch) => ch.receive().await,
            }
        }
    }
}

impl legacy_common::platform::L2capSender for L2capTxWrapper {
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
