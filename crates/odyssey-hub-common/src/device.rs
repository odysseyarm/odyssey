#[repr(u8)]
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// Wired USB.
    Usb,
    /// BLE through a mux dongle.
    UsbMux,
    /// UDP through a mux. Not implemented.
    UdpMux,
}

/// Bit flags for the operations a device supports.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceCapabilities {
    bits: u8,
}

impl DeviceCapabilities {
    /// USB control interface: pairing, config, DFU.
    pub const CONTROL: u8 = 0b01;
    /// Packet and event streams.
    pub const EVENTS: u8 = 0b10;
    /// Firmware flashing is available without the control protocol.
    /// Bootloader entries report `[0, 0, 0]`; DFU-only applications also set `DFU_RUNTIME_ONLY`.
    pub const DFU: u8 = 0b100;
    /// `netcore_release` is a settled reading, not a read still in progress.
    /// Without this flag a 0 can also mean "not answered yet", which a unit shows for a
    /// while after every boot, so such a 0 must not be judged as behind.
    pub const NETCORE_KNOWN: u8 = 0b1000;
    /// The firmware picks its data link itself: USB while plugged into a computer,
    /// Bluetooth otherwise. There is no transport mode to choose, and
    /// `SetTransportMode` only reports the link in use.
    pub const AUTO_TRANSPORT: u8 = 0b1_0000;
    /// Application exposes USB DFU runtime but no Odyssey control or event interfaces.
    pub const DFU_RUNTIME_ONLY: u8 = 0b10_0000;

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

/// How the device sends events and sensor data.
#[repr(u8)]
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventsTransport {
    /// Over USB.
    Wired,
    /// Over BLE.
    Bluetooth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct Device {
    pub uuid: [u8; 6],
    pub transport: Transport,
    pub capabilities: DeviceCapabilities,
    /// `[major, minor, patch]`, `[0, 0, 0]` if unknown.
    pub firmware_version: [u16; 3],
    pub events_transport: EventsTransport,
    /// True while events and sensor data are arriving.
    pub events_connected: bool,
    /// USB application PID (0x520F VM, 0x5210 Lite, 0x5211 Lite1, 0x5212 Mux, 0x5213 marker board), 0 if unknown.
    pub product_id: u16,
    /// NUL-terminated UTF-8, at most 32 bytes plus the NUL. C layout: `uint8_t name[33]`.
    pub name: [u8; 33],
    /// Release counter of the nRF5340 net core (BLE controller) firmware, 0 if unknown.
    ///
    /// Unknown covers a device whose application predates the property, a net core
    /// that does not report one, and a read that has not completed yet. A 0 only
    /// means "reported no release" once [`DeviceCapabilities::NETCORE_KNOWN`] is set.
    pub netcore_release: u16,
}

impl Device {
    /// A `[u8; 33]` name buffer holding the first 32 bytes of `s`, for struct literals.
    pub fn name_bytes(s: &str) -> [u8; 33] {
        let mut buf = [0u8; 33];
        let bytes = s.as_bytes();
        let len = bytes.len().min(32);
        buf[..len].copy_from_slice(&bytes[..len]);
        buf
    }

    pub fn name(&self) -> &str {
        let nul = self.name.iter().position(|&b| b == 0).unwrap_or(33);
        std::str::from_utf8(&self.name[..nul]).unwrap_or("")
    }

    pub fn set_name(&mut self, s: &str) {
        self.name = [0u8; 33];
        let bytes = s.as_bytes();
        let len = bytes.len().min(32);
        self.name[..len].copy_from_slice(&bytes[..len]);
    }

    /// True if both snapshots have the same `uuid`, meaning the same physical device,
    /// whatever their name, capabilities, firmware version or connection state.
    ///
    /// Use it instead of `==` for identity lookups, such as matching the `Device` in an
    /// event against the known device list.
    pub fn uuid_eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}
