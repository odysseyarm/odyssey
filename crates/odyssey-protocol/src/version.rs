use odyssey_protocol_macros::protocol_type;

/// Version of the device wire format, reported by a device in
/// [`Props::Version`] and over the control endpoint. Bump when packet shape or
/// accepted value ranges change.
///
/// This is a separate axis from the crate version in `Cargo.toml`. The crate
/// version follows semver for the Rust API: a change that breaks callers (a
/// new variant on an exhaustive enum, a removed item) bumps it even when no
/// packet changed. Version 0.2.0 widens firmware version components from `u16`
/// to `u32`. CBOR arrays keep the same shape, but large values need new consumers.
///
/// Since: Odyssey Desktop 0.6.0.
pub const DEVICE_PROTOCOL: [u16; 3] = [0, 2, 0];

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Version {
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub protocol_semver: [u16; 3],
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub firmware_semver: [u32; 3],
}

impl Version {
    /// Since: Odyssey Desktop 0.6.0.
    pub fn new(firmware_semver: [u32; 3]) -> Self {
        Self {
            protocol_semver: DEVICE_PROTOCOL,
            firmware_semver,
        }
    }
}

#[cfg(all(test, feature = "minicbor"))]
mod tests {
    use super::Version;

    #[test]
    fn firmware_version_roundtrips_components_above_u16() {
        let version = Version::new([u32::MAX, 65_536, 1]);
        let mut bytes = [0; 40];
        let mut encoder = minicbor::Encoder::new(&mut bytes[..]);
        encoder.encode(version).unwrap();
        let encoded_len = 40 - encoder.writer().len();
        let decoded: Version = minicbor::decode(&bytes[..encoded_len]).unwrap();
        assert_eq!(decoded.firmware_semver, [u32::MAX, 65_536, 1]);
    }
}
