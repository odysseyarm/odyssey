use std::fmt;
use std::net::SocketAddr;

#[derive(Clone, PartialEq, Eq, Hash)]
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

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Device::Udp(d) => f.debug_tuple("Udp").field(d).finish(),
            Device::Hid(d) => f.debug_tuple("Hid").field(d).finish(),
            Device::Cdc(d) => f.debug_tuple("Cdc").field(d).finish(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct UdpDevice {
    pub uuid: u64,
    pub id: u8,
    pub addr: SocketAddr,
}

impl fmt::Debug for UdpDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpDevice")
            .field("uuid", &format_args!("0x{:x}", self.uuid))
            .field("id", &self.id)
            .field("addr", &self.addr)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct HidDevice {
    pub uuid: u64,
    pub path: String,
}

impl fmt::Debug for HidDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HidDevice")
            .field("uuid", &format_args!("0x{:x}", self.uuid))
            .field("path", &self.path)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CdcDevice {
    pub uuid: u64,
    pub path: String,
}

impl fmt::Debug for CdcDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdcDevice")
            .field("uuid", &format_args!("0x{:x}", self.uuid))
            .field("path", &self.path)
            .finish()
    }
}
