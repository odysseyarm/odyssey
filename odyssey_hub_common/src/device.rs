use std::net::SocketAddr;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    Udp(UdpDevice),
    Hid(HidDevice),
    Cdc(CdcDevice),
}

impl Device {
    pub fn uuid(&self) -> u64 {
        match self {
            Device::Udp(d) => d.uuid,
            Device::Hid(d) => d.uuid,
            Device::Cdc(d) => d.uuid,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UdpDevice {
    pub uuid: u64,
    pub id: u8,
    pub addr: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HidDevice {
    pub uuid: u64,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CdcDevice {
    pub uuid: u64,
    pub path: String,
}
