use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub enum Device {
    Udp((u8, SocketAddr)),
    Hid(String),
    Cdc(String),
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
