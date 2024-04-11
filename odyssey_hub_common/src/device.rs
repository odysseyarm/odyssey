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

#[derive(Clone, Copy, Debug)]
pub struct UdpDevice {
    pub id: u8,
    pub addr: SocketAddr,
}

impl PartialEq for UdpDevice {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.addr == other.addr
    }
}

#[derive(Clone, Debug)]
pub struct HidDevice {
    pub path: String,
}

impl PartialEq for HidDevice {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    } 
}

#[derive(Clone, Debug)]
pub struct CdcDevice {
    pub path: String,
}

impl PartialEq for CdcDevice {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}
