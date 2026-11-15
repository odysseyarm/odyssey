// The VM has no BLE: receivers pend forever and senders drop packets as if
// they were delivered.

use core::future::Future;

use legacy_common::odyssey_protocol::Packet;

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy)]
pub struct VmL2capChannels {
    /// Since: Odyssey Desktop 0.7.0.
    pub control_rx: MockReceiver,
    /// Since: Odyssey Desktop 0.7.0.
    pub data_rx: MockReceiver,
    /// Since: Odyssey Desktop 0.7.0.
    pub control_tx: MockSender,
    /// Since: Odyssey Desktop 0.7.0.
    pub data_tx: MockSender,
}

impl VmL2capChannels {
    /// Since: Odyssey Desktop 0.7.0.
    pub const fn new() -> Self {
        Self {
            control_rx: MockReceiver,
            data_rx: MockReceiver,
            control_tx: MockSender,
            data_tx: MockSender,
        }
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy)]
pub struct MockReceiver;

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy)]
pub struct MockSender;

impl legacy_common::platform::L2capChannels for VmL2capChannels {
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

    fn session_generation(&self) -> u32 {
        // This board has no BLE connection lifecycle.
        0
    }
}

impl legacy_common::platform::L2capReceiver for MockReceiver {
    fn receive(&self) -> impl Future<Output = Packet> {
        async move { core::future::pending().await }
    }
}

impl legacy_common::platform::L2capSender for MockSender {
    fn send(&self, _pkt: Packet) -> impl Future<Output = ()> {
        async move {
            // The packet is dropped. The VM has no BLE.
        }
    }

    fn try_send(&self, _pkt: Packet) -> Result<(), ()> {
        // The packet is dropped. The VM has no BLE.
        Ok(())
    }
}
