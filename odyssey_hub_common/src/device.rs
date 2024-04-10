use std::net::SocketAddr;

#[derive(Debug, Clone, Copy)]
pub enum Device {
    Udp((SocketAddr, u8)),
    Hid,
    Cdc,
}

impl PartialEq for Device {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Device::Udp(a), Device::Udp(b)) => a == b,
            (Device::Hid, Device::Hid) => true,
            (Device::Cdc, Device::Cdc) => true,
            _ => false,
        }
    }
}
