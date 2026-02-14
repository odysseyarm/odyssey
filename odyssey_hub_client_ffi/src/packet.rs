use ats_usb::packets::vm::VendorData;

#[repr(C)]
#[derive(Clone)]
pub struct PacketEvent {
    pub ty: u8,
    pub data: PacketData,
}

#[repr(C)]
#[derive(Clone)]
pub enum PacketData {
    Unsupported,
    VendorEvent(VendorEventPacketData),
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VendorEventPacketData {
    pub len: u8,
    pub data: [u8; 98],
}

impl From<ats_usb::packets::vm::Packet> for PacketEvent {
    fn from(p: ats_usb::packets::vm::Packet) -> Self {
        let ty: u8 = p.ty().into();
        let data = match p.data {
            ats_usb::packets::vm::PacketData::Vendor(_, VendorData { len, data }) => {
                PacketData::VendorEvent(VendorEventPacketData { len, data })
            }
            _ => PacketData::Unsupported,
        };
        Self { ty, data }
    }
}
