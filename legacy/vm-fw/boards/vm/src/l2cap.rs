// Mock L2CAP implementation for VM (no BLE support)
// All operations are no-ops or pend forever

use common::protodongers::Packet;
use core::future::Future;

#[derive(Clone, Copy)]
pub struct VmL2capChannels {
    pub control_rx: MockReceiver,
    pub data_rx: MockReceiver,
    pub control_tx: MockSender,
    pub data_tx: MockSender,
}

impl VmL2capChannels {
    pub const fn new() -> Self {
        Self {
            control_rx: MockReceiver,
            data_rx: MockReceiver,
            control_tx: MockSender,
            data_tx: MockSender,
        }
    }
}

#[derive(Clone, Copy)]
pub struct MockReceiver;

#[derive(Clone, Copy)]
pub struct MockSender;

// Implement the L2capChannels trait
impl common::platform::L2capChannels for VmL2capChannels {
    type Receiver = MockReceiver;
    type Sender = MockSender;

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
}

// Implement the L2capReceiver trait - always pend
impl common::platform::L2capReceiver for MockReceiver {
    fn receive(&self) -> impl Future<Output = Packet> {
        async move {
            // Never actually receive anything (VM has no BLE)
            core::future::pending().await
        }
    }
}

// Implement the L2capSender trait - always succeed (discard)
impl common::platform::L2capSender for MockSender {
    fn send(&self, _pkt: Packet) -> impl Future<Output = ()> {
        async move {
            // VM has no BLE, so pretend the packet was delivered immediately.
        }
    }

    fn try_send(&self, _pkt: Packet) -> Result<(), ()> {
        // Pretend we sent it successfully
        Ok(())
    }
}
