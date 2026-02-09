use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use protodongers::Packet;
use static_cell::StaticCell;

/// Channel for incoming CONTROL packets from L2CAP (BLE -> Main)
/// Size: 16 packets - matches L2CAP initial credits
pub type L2capControlRxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Channel for incoming DATA packets from L2CAP (BLE -> Main)
/// Size: 16 packets - buffer for streaming data
pub type L2capDataRxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Channel for outgoing CONTROL packets (dedicated L2CAP channel)
/// Size: 16 packets to handle burst queries without excessive memory
pub type L2capControlTxChannel = Channel<CriticalSectionRawMutex, Packet, 16>;

/// Channel for outgoing DATA packets (dedicated L2CAP channel)
/// Size: 8 packets - data should be consumed quickly, minimize memory usage
pub type L2capDataTxChannel = Channel<CriticalSectionRawMutex, Packet, 8>;

/// Global L2CAP Control RX channel
static L2CAP_CONTROL_RX: StaticCell<L2capControlRxChannel> = StaticCell::new();

/// Global L2CAP Data RX channel
static L2CAP_DATA_RX: StaticCell<L2capDataRxChannel> = StaticCell::new();

/// Global L2CAP Control TX channel
static L2CAP_CONTROL_TX: StaticCell<L2capControlTxChannel> = StaticCell::new();

/// Global L2CAP Data TX channel
static L2CAP_DATA_TX: StaticCell<L2capDataTxChannel> = StaticCell::new();

/// Track if channels have been initialized
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Stored references to initialized channels
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

/// Initialize or get the L2CAP bridge channels
/// Safe to call multiple times - only initializes once
pub fn get_or_init() -> L2capChannels {
    if !INITIALIZED.load(Ordering::Acquire) {
        // First call - initialize
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

    // Return stored references
    unsafe {
        L2capChannels {
            control_rx: CONTROL_RX_REF.unwrap(),
            data_rx: DATA_RX_REF.unwrap(),
            control_tx: CONTROL_TX_REF.unwrap(),
            data_tx: DATA_TX_REF.unwrap(),
        }
    }
}

/// Helper to determine if a packet is a control packet (vs streaming data)
pub fn is_control_packet(pkt: &Packet) -> bool {
    use protodongers::PacketData as P;

    match &pkt.data {
        // Control packets - commands and responses
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
        | P::Vendor(_, _) => true,

        // Streaming data packets
        P::ObjectReport(_)
        | P::CombinedMarkersReport(_)
        | P::PocMarkersReport(_)
        | P::AccelReport(_)
        | P::ImpactReport(_) => false,
    }
}
