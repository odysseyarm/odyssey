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

/// Device capabilities indicate what operations are available for a device
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceCapabilities {
    bits: u8,
}

impl DeviceCapabilities {
    /// Device has USB control interface (pairing, config, DFU)
    pub const CONTROL: u8 = 0b01;
    /// Device has packet/event streams
    pub const EVENTS: u8 = 0b10;

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn new(bits: u8) -> Self {
        Self { bits }
    }

    pub const fn contains(&self, flag: u8) -> bool {
        (self.bits & flag) == flag
    }

    pub fn insert(&mut self, flag: u8) {
        self.bits |= flag;
    }

    pub fn remove(&mut self, flag: u8) {
        self.bits &= !flag;
    }

    pub const fn bits(&self) -> u8 {
        self.bits
    }
}

/// How events/sensor data are transmitted from the device
#[repr(u8)]
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventsTransport {
    /// Events transmitted via wired USB connection
    Wired,
    /// Events transmitted via BLE (wireless)
    Bluetooth,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Device {
    pub uuid: [u8; 6],
    pub transport: Transport,
    pub capabilities: DeviceCapabilities,
    pub firmware_version: Option<[u16; 3]>,
    pub events_transport: EventsTransport,
    /// Whether events/sensor data are currently being received
    pub events_connected: bool,
    /// USB Product ID (e.g. 0x520F=AtsVm, 0x5210=AtsLite, 0x5211=Lite1, 0x5212=Mux)
    pub product_id: Option<u16>,
}
