use odyssey_usb::packets::vm::VendorData;

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone)]
#[derive(Debug)]
pub struct PacketEvent {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub ty: u8,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub data: PacketData,
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone)]
#[derive(Debug)]
pub enum PacketData {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    Unsupported,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    VendorEvent(VendorEventPacketData),
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone, Copy)]
#[derive(Debug)]
pub struct VendorEventPacketData {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub len: u8,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub data: [u8; 98],
}

impl From<odyssey_usb::packets::vm::Packet> for PacketEvent {
    fn from(p: odyssey_usb::packets::vm::Packet) -> Self {
        let ty: u8 = p.ty().into();
        let data = match p.data {
            odyssey_usb::packets::vm::PacketData::Vendor(_, VendorData { len, data }) => {
                PacketData::VendorEvent(VendorEventPacketData { len, data })
            }
            _ => PacketData::Unsupported,
        };
        Self { ty, data }
    }
}
