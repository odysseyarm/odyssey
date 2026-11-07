//! BLE central built on trouble-host (pure Rust host) over nrf-sdc.

use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;

pub mod accessories;
pub mod central;
pub mod device_task;
pub mod pairing_scanner;
pub mod security;
pub mod task_slots;

pub use central::{ACTIVE_CONNECTIONS, ActiveConnections, BleManager};

/// Packets from BLE devices on their way to USB. Depth 32 rather than 64 to
/// save RAM.
pub type DevicePacketChannel = Channel<ThreadModeRawMutex, odyssey_protocol::mux::DevicePacket, 32>;
// Per-device queues and connection tracking live in `ble::central`.
