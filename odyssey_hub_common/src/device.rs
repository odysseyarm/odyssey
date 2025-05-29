use std::net::SocketAddr;

#[derive(Clone, Debug, Eq, Hash)]
pub enum Device {
    Udp(UdpDevice),
    Hid(HidDevice),
    Cdc(CdcDevice),
}

impl Device {
    pub fn uuid(&self) -> [u8; 6] {
        match self {
            Device::Udp(d) => d.uuid,
            Device::Cdc(d) => d.uuid,
            Device::Hid(d) => d.uuid,
        }
    }
}

impl PartialEq for Device {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Device::Udp(a), Device::Udp(b)) => a.addr == b.addr && a.id == b.id,
            (Device::Hid(a), Device::Hid(b)) => a.path == b.path,
            (Device::Cdc(a), Device::Cdc(b)) => a.path == b.path,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash)]
pub struct UdpDevice {
    pub id: u8,
    pub addr: SocketAddr,
    pub uuid: [u8; 6],
}

impl PartialEq for UdpDevice {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (a, b) => a.addr == b.addr && a.id == b.id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HidDevice {
    pub path: String,
    pub uuid: [u8; 6],
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CdcDevice {
    pub path: String,
    pub uuid: [u8; 6],
}
