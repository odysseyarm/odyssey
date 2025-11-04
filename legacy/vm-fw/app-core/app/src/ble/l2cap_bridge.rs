use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use protodongers::Packet;
use static_cell::StaticCell;

/// Channel for incoming packets from L2CAP (BLE -> Main)
pub type L2capRxChannel = Channel<ThreadModeRawMutex, Packet, 64>;

/// Channel for outgoing packets to L2CAP (Main -> BLE)
/// Buffer sized for 300 pkt/s (100Hz IMU + 200Hz vision) with ~213ms buffering
pub type L2capTxChannel = Channel<ThreadModeRawMutex, Packet, 64>;

/// Global L2CAP RX channel
static L2CAP_RX: StaticCell<L2capRxChannel> = StaticCell::new();

/// Global L2CAP TX channel
static L2CAP_TX: StaticCell<L2capTxChannel> = StaticCell::new();

/// Track if channels have been initialized
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Stored references to initialized channels
static mut RX_REF: Option<&'static L2capRxChannel> = None;
static mut TX_REF: Option<&'static L2capTxChannel> = None;

/// Initialize or get the L2CAP bridge channels
/// Safe to call multiple times - only initializes once
pub fn get_or_init() -> (&'static L2capRxChannel, &'static L2capTxChannel) {
    if !INITIALIZED.load(Ordering::Acquire) {
        // First call - initialize
        let rx = L2CAP_RX.init(L2capRxChannel::new());
        let tx = L2CAP_TX.init(L2capTxChannel::new());

        unsafe {
            RX_REF = Some(rx);
            TX_REF = Some(tx);
        }

        INITIALIZED.store(true, Ordering::Release);
    }

    // Return stored references
    unsafe { (RX_REF.unwrap(), TX_REF.unwrap()) }
}
