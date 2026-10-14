//! USB initialization for lite1 board
//!
//! Re-exports the common USB implementation from legacy/vm-fw/common.
//! Uses vendor-specific bulk endpoints with vendor control requests (0x30/0x31).

// The USB functionality is provided by common::usb
// This file exists for board-specific USB constants if needed

pub const PID: u16 = 0x5211; // lite1 PID
pub const DEVICE_INTERFACE_GUID: &str = "{A4769731-EC56-49FF-9924-613E5B3D4D6C}";
