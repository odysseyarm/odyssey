use odyssey_protocol_macros::protocol_type;

/// Version of the device wire format, reported by a device in
/// [`Props::Version`] and over the control endpoint. Bump it when the packets
/// in this module change shape.
///
/// This is a separate axis from the crate version in `Cargo.toml`. The crate
/// version follows semver for the Rust API: a change that breaks callers (a
/// new variant on an exhaustive enum, a removed item) bumps it even when no
/// packet changed. This constant moves only when the bytes on the wire do, so
/// a refactor-only release does not tell every device in the field that the
/// protocol moved. Host-side `semver_at_least` gates read this value.
///
/// 0.1.6: the device picks its transport itself (USB while a USB host is
/// attached, BLE otherwise) and `SetTransportMode` no longer reboots or
/// persists anything. The packets are unchanged; their meaning is not.
pub const DEVICE_PROTOCOL: [u16; 3] = [0, 1, 6];

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Version {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub protocol_semver: [u16; 3],
    #[cfg_attr(feature = "minicbor", n(1))]
    pub firmware_semver: [u16; 3],
}

impl Version {
    pub fn new(firmware_semver: [u16; 3]) -> Self {
        Self {
            protocol_semver: DEVICE_PROTOCOL,
            firmware_semver,
        }
    }
}
