#[repr(u8)]
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// Direct USB connection
    Usb,
    /// BLE connection via mux/dongle
    UsbMux,
    /// UDP connection via mux (not yet implemented)
    UdpMux,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Device {
    pub uuid: [u8; 6],
    pub transport: Transport,
}
