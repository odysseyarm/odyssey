//! BLE module using trouble-host + nrf-sdc
//!
//! This replaces the previous nrf-softdevice implementation with trouble-host (pure Rust BLE host).

use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;

pub mod central;
pub mod device_task;
pub mod pairing_scanner;
pub mod security;

pub use central::{ACTIVE_CONNECTIONS, ActiveConnections, BleManager};

/// Channel for sending packets from BLE devices to USB
/// Reduced to 32 to save RAM (was 64)
pub type DevicePacketChannel = Channel<ThreadModeRawMutex, protodongers::mux::DevicePacket, 32>;
// Device queues and connection tracking are defined in `ble::central`.
