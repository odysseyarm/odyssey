use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, TrySendError};
use odyssey_protocol::Packet;

/// Control packets received over L2CAP. 16 deep to match the initial L2CAP credits.
///
/// Since: Odyssey Desktop 0.7.0.
pub type L2capControlRxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Streaming packets received over L2CAP.
///
/// Since: Odyssey Desktop 0.7.0.
pub type L2capDataRxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Control packets to send on the control L2CAP channel. 16 deep absorbs a burst of queries.
///
/// Since: Odyssey Desktop 0.7.0.
pub type L2capControlTxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Streaming packets to send on the data L2CAP channel. Kept at 8 to save RAM. The sender
/// drains it quickly.
///
/// Since: Odyssey Desktop 0.7.0.
pub type L2capDataTxChannel = Channel<CriticalSectionRawMutex, Packet, 8>;

static L2CAP_CONTROL_RX: L2capControlRxChannel = Channel::new();
static L2CAP_DATA_RX: L2capDataRxChannel = Channel::new();
static L2CAP_CONTROL_TX: L2capControlTxChannel = Channel::new();
static L2CAP_DATA_TX: L2capDataTxChannel = Channel::new();

static SESSION_GENERATION: AtomicU32 = AtomicU32::new(0);

/// Changes whenever a BLE session starts or ends, including while a request handler awaits I/O.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn session_generation() -> u32 {
    SESSION_GENERATION.load(Ordering::Relaxed)
}

/// Since: Odyssey Desktop 0.7.0.
pub struct L2capChannels {
    /// Since: Odyssey Desktop 0.7.0.
    pub control_rx: &'static L2capControlRxChannel,
    /// Since: Odyssey Desktop 0.7.0.
    pub data_rx: &'static L2capDataRxChannel,
    /// Since: Odyssey Desktop 0.7.0.
    pub control_tx: &'static L2capControlTxChannel,
    /// Since: Odyssey Desktop 0.7.0.
    pub data_tx: &'static L2capDataTxChannel,
}

impl L2capChannels {
    /// Queues `pkt` for the central on the control or the data channel, by packet kind.
    ///
    /// # Errors
    ///
    /// Returns the packet when that queue is full.
    ///
    /// # Since
    /// Odyssey Desktop 0.7.0.
    pub fn try_send(&self, pkt: Packet) -> Result<(), TrySendError<Packet>> {
        if pkt.is_control() {
            self.control_tx.try_send(pkt)
        } else {
            self.data_tx.try_send(pkt)
        }
    }

    /// Invalidates in-flight replies and empties all four queues at each BLE session boundary,
    /// so a central never receives packets queued or still being handled for an earlier session.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn drain(&self) {
        SESSION_GENERATION.fetch_add(1, Ordering::Relaxed);
        self.control_rx.clear();
        self.data_rx.clear();
        self.control_tx.clear();
        self.data_tx.clear();
    }
}

/// Returns the statically allocated L2CAP bridge channels.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn channels() -> L2capChannels {
    L2capChannels {
        control_rx: &L2CAP_CONTROL_RX,
        data_rx: &L2CAP_DATA_RX,
        control_tx: &L2CAP_CONTROL_TX,
        data_tx: &L2CAP_DATA_TX,
    }
}
