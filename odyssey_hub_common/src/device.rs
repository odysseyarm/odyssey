#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Transport {
    /// Direct USB connection
    USB,
    /// BLE connection via USB Hub/Dongle
    BLE,
    /// UDP connection via Hub (not yet implemented)
    UDPHub,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Device {
    pub uuid: u64,
    pub transport: Transport,
}
