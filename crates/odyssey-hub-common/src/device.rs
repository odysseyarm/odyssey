use crate::firmware_version::FirmwareVersion;

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(u8)]
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// Wired USB.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    Usb,
    /// BLE through a mux dongle.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    UsbMux,
    /// UDP through a mux. Not implemented.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    UdpMux,
}

/// Bit flags for the operations a device supports.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceCapabilities {
    bits: u8,
}

impl DeviceCapabilities {
    /// USB control interface: pairing, config, DFU.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub const CONTROL: u8 = 0b01;
    /// Packet and event streams.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub const EVENTS: u8 = 0b10;
    /// Firmware flashing is available without the control protocol.
    /// Bootloader entries report `[0, 0, 0]`; DFU-only applications also set `DFU_RUNTIME_ONLY`.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub const DFU: u8 = 0b100;
    /// `netcore_release` is a settled reading, not a read still in progress.
    /// Without this flag a 0 can also mean "not answered yet", which a unit shows for a
    /// while after every boot, so such a 0 must not be judged as behind.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub const NETCORE_KNOWN: u8 = 0b1000;
    /// The firmware picks its data link itself: USB while plugged into a computer,
    /// Bluetooth otherwise. There is no transport mode to choose, and
    /// `SetTransportMode` only reports the link in use.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub const AUTO_TRANSPORT: u8 = 0b1_0000;
    /// Application exposes USB DFU runtime but no Odyssey control or event interfaces.
    ///
    /// Since: Odyssey Desktop 0.7.4.
    pub const DFU_RUNTIME_ONLY: u8 = 0b10_0000;

    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub const fn new(bits: u8) -> Self {
        Self { bits }
    }

    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub const fn contains(&self, flag: u8) -> bool {
        (self.bits & flag) == flag
    }

    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub fn insert(&mut self, flag: u8) {
        self.bits |= flag;
    }

    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub fn remove(&mut self, flag: u8) {
        self.bits &= !flag;
    }

    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub const fn bits(&self) -> u8 {
        self.bits
    }
}

/// How the device sends events and sensor data.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(u8)]
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventsTransport {
    /// Over USB.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    Wired,
    /// Over BLE.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    Bluetooth,
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct Device {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub uuid: [u8; 6],
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub transport: Transport,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub capabilities: DeviceCapabilities,
    /// The version the device's USB descriptor reports, channel `Unknown` if it reported none.
    ///
    /// Its numeric core can differ from `firmware_version`: a runtime-only DFU
    /// application reports only its major there.
    ///
    /// Since: Odyssey Desktop (unreleased).
    pub firmware_identity: FirmwareVersion,
    /// `[major, minor, patch]`, `[0, 0, 0]` if unknown.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub firmware_version: [u32; 3],
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub events_transport: EventsTransport,
    /// True while events and sensor data are arriving.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub events_connected: bool,
    /// USB application PID (0x520F VM, 0x5210 Lite, 0x5211 Lite1, 0x5212 Mux, 0x5213 marker board), 0 if unknown.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub product_id: u16,
    /// NUL-terminated UTF-8, at most 32 bytes plus the NUL. C layout: `uint8_t name[33]`.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub name: [u8; 33],
    /// Release counter of the nRF5340 net core (BLE controller) firmware, 0 if unknown.
    ///
    /// Unknown covers a device whose application predates the property, a net core
    /// that does not report one, and a read that has not completed yet. A 0 only
    /// means "reported no release" once [`DeviceCapabilities::NETCORE_KNOWN`] is set.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    pub netcore_release: u16,
}

impl Device {
    /// A `[u8; 33]` name buffer holding the first 32 bytes of `s`, for struct literals.
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub fn name_bytes(s: &str) -> [u8; 33] {
        let mut buf = [0u8; 33];
        let bytes = s.as_bytes();
        let len = bytes.len().min(32);
        buf[..len].copy_from_slice(&bytes[..len]);
        buf
    }

    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub fn name(&self) -> &str {
        let nul = self.name.iter().position(|&b| b == 0).unwrap_or(33);
        std::str::from_utf8(&self.name[..nul]).unwrap_or("")
    }

    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
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
    ///
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub fn uuid_eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}
