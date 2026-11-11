mod cobs;
mod control_transfer;
mod general_settings;
mod hex_address;
mod mux_accessory;
mod mux_device;
mod mux_transport;
mod packet_stream;
mod packet_transport;
mod response_slots;
mod retry;
mod usb_bulk_io;
mod vm_connection_info;
mod vm_device;

/// Since: Odyssey Desktop 0.6.0.
pub use cobs::COBS_DELIMITER;
/// Since: Odyssey Desktop 0.6.0.
pub use general_settings::GeneralSettings;
/// Since: Odyssey Desktop 0.6.0.
pub use mux_device::{MuxDevice, PairingEvent};
/// Since: Odyssey Desktop 0.6.0.
pub use mux_transport::MuxTransport;
/// Since: Odyssey Desktop 0.6.0.
pub use odyssey_protocol::ProductId;
/// Since: Odyssey Desktop 0.6.0.
pub use packet_stream::PacketStream;
/// Since: Odyssey Desktop 0.6.0.
pub use packet_transport::{PacketTransport, PacketTransportTx};
/// Since: Odyssey Desktop 0.6.0.
pub use vm_connection_info::VmConnectionInfo;
/// Since: Odyssey Desktop 0.6.0.
pub use vm_device::VmDevice;
