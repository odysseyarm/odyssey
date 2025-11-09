//! BLE module using trouble-host + nrf-sdc
//!
//! This replaces the previous nrf-softdevice implementation with trouble-host (pure Rust BLE host).

use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;

pub mod central;
pub mod security;

pub use central::{ACTIVE_CONNECTIONS, ActiveConnections, BleManager};

/// Channel for sending packets from BLE devices to USB
/// Size: 32 packets to handle burst traffic at 7.5ms intervals without dropping control packets
pub type DevicePacketChannel = Channel<ThreadModeRawMutex, protodongers::mux::DevicePacket, 32>;
// Device queues and connection tracking are defined in `ble::central`.
