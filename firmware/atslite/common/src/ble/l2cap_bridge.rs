use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use odyssey_protocol::Packet;
use static_cell::StaticCell;

/// Control packets received over L2CAP. 16 deep to match the initial L2CAP credits.
pub type L2capControlRxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Streaming packets received over L2CAP.
pub type L2capDataRxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Control packets to send on the control L2CAP channel. 16 deep absorbs a burst of queries.
pub type L2capControlTxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Streaming packets to send on the data L2CAP channel. Kept at 8 to save RAM; the sender
/// drains it quickly.
pub type L2capDataTxChannel = Channel<CriticalSectionRawMutex, Packet, 8>;

static L2CAP_CONTROL_RX: StaticCell<L2capControlRxChannel> = StaticCell::new();

static L2CAP_DATA_RX: StaticCell<L2capDataRxChannel> = StaticCell::new();

static L2CAP_CONTROL_TX: StaticCell<L2capControlTxChannel> = StaticCell::new();

static L2CAP_DATA_TX: StaticCell<L2capDataTxChannel> = StaticCell::new();

static INITIALIZED: AtomicBool = AtomicBool::new(false);

static mut CONTROL_RX_REF: Option<&'static L2capControlRxChannel> = None;
static mut DATA_RX_REF: Option<&'static L2capDataRxChannel> = None;
static mut CONTROL_TX_REF: Option<&'static L2capControlTxChannel> = None;
static mut DATA_TX_REF: Option<&'static L2capDataTxChannel> = None;

pub struct L2capChannels {
    pub control_rx: &'static L2capControlRxChannel,
    pub data_rx: &'static L2capDataRxChannel,
    pub control_tx: &'static L2capControlTxChannel,
    pub data_tx: &'static L2capDataTxChannel,
}

impl L2capChannels {
    /// Empties all four queues. A new BLE session starts with this, so the central does not get
    /// replies and reports queued for an earlier session (or while no central was connected),
    /// and requests left from an earlier session are not answered on this one.
    pub fn drain(&self) {
        self.control_rx.clear();
        self.data_rx.clear();
        self.control_tx.clear();
        self.data_tx.clear();
    }
}

/// Returns the L2CAP bridge channels, creating them on the first call.
pub fn get_or_init() -> L2capChannels {
    if !INITIALIZED.load(Ordering::Acquire) {
        let control_rx = L2CAP_CONTROL_RX.init(L2capControlRxChannel::new());
        let data_rx = L2CAP_DATA_RX.init(L2capDataRxChannel::new());
        let control_tx = L2CAP_CONTROL_TX.init(L2capControlTxChannel::new());
        let data_tx = L2CAP_DATA_TX.init(L2capDataTxChannel::new());

        unsafe {
            CONTROL_RX_REF = Some(control_rx);
            DATA_RX_REF = Some(data_rx);
            CONTROL_TX_REF = Some(control_tx);
            DATA_TX_REF = Some(data_tx);
        }

        INITIALIZED.store(true, Ordering::Release);
    }

    unsafe {
        L2capChannels {
            control_rx: CONTROL_RX_REF.unwrap(),
            data_rx: DATA_RX_REF.unwrap(),
            control_tx: CONTROL_TX_REF.unwrap(),
            data_tx: DATA_TX_REF.unwrap(),
        }
    }
}

/// True for commands and responses, false for streaming reports.
pub fn is_control_packet(pkt: &Packet) -> bool {
    use odyssey_protocol::PacketData as P;

    match &pkt.data {
        P::StreamUpdate(_)
        | P::WriteMode(_)
        | P::ReadVersion()
        | P::ReadVersionResponse(_)
        | P::ReadConfig(_)
        | P::ReadConfigResponse(_)
        | P::WriteConfig(_)
        | P::ReadProp(_)
        | P::ReadPropResponse(_)
        | P::WriteRegister(_)
        | P::ReadRegister(_)
        | P::ReadRegisterResponse(_)
        | P::ObjectReportRequest()
        | P::Ack()
        | P::FlashSettings()
        | P::Vendor(_, _)
        | P::SetDeviceName(_)
        | P::SetDeviceNameResponse(_) => true,

        P::ObjectReport(_)
        | P::CombinedMarkersReport(_)
        | P::PocMarkersReport(_)
        | P::AccelReport(_)
        | P::ImpactReport(_)
        | P::BatteryReport(_) => false,
    }
}
