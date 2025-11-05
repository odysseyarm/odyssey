#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Transport {
    /// Direct USB connection
    Usb,
    /// BLE connection via USB Hub/Dongle
    UsbHub,
    /// UDP connection via Hub (not yet implemented)
    UdpHub,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Device {
    pub uuid: [u8; 6],
    pub transport: Transport,
}
