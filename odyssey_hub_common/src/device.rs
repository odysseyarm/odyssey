use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub enum Device {
    Udp(UdpDevice),
    Hid(HidDevice),
    Cdc(CdcDevice),
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

#[derive(Clone, Copy, Debug, Eq)]
pub struct UdpDevice {
    pub id: u8,
    pub addr: SocketAddr,
    pub uuid: [u8; 6],
}

impl PartialEq for UdpDevice {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (a, b) => a.addr == b.addr && a.id == b.id,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidDevice {
    pub path: String,
    pub uuid: [u8; 6],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdcDevice {
    pub path: String,
    pub uuid: [u8; 6],
}
