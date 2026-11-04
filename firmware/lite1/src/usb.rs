//! USB identifiers for lite1. The USB stack is the shared common::usb, which
//! uses vendor-specific bulk endpoints and vendor control requests 0x30/0x31.

pub const PID: u16 = 0x5211;
pub const DEVICE_INTERFACE_GUID: &str = "{A4769731-EC56-49FF-9924-613E5B3D4D6C}";
