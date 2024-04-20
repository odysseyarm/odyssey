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
            (Device::Udp(a), Device::Udp(b)) => a == b,
            (Device::Hid(a), Device::Hid(b)) => a == b,
            (Device::Cdc(a), Device::Cdc(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UdpDevice {
    pub id: u8,
    pub addr: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidDevice {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdcDevice {
    pub path: String,
}
