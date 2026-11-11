use odyssey_hub_server_interface as proto;

/// Conversion rejects malformed hardware addresses and overflowing protocol version fields.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[derive(Clone, Debug, PartialEq)]
pub struct DongleInfo {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub id: String,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub protocol_version: [u16; 3], // mux endpoint protocol
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub control_protocol_version: [u16; 3], // control endpoint protocol
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub firmware_version: [u32; 3],
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub connected_devices: Vec<[u8; 6]>,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub bonded_devices: Vec<[u8; 6]>,
}

impl TryFrom<proto::Dongle> for DongleInfo {
    type Error = &'static str;

    fn try_from(dongle: proto::Dongle) -> Result<Self, Self::Error> {
        Ok(Self {
            id: dongle.id,
            protocol_version: protocol_version_triplet(dongle.protocol_version)?,
            control_protocol_version: protocol_version_triplet(dongle.control_protocol_version)?,
            firmware_version: firmware_version_triplet(dongle.firmware_version),
            connected_devices: addresses(&dongle.connected_devices)?,
            bonded_devices: addresses(&dongle.bonded_devices)?,
        })
    }
}

/// A protocol version the dongle did not report is `[0, 0, 0]`.
fn protocol_version_triplet(
    version: Option<proto::FirmwareVersion>,
) -> Result<[u16; 3], &'static str> {
    let Some(version) = version else {
        return Ok([0; 3]);
    };
    Ok([
        u16::try_from(version.major).map_err(|_conversion_error| "protocol major exceeds u16")?,
        u16::try_from(version.minor).map_err(|_conversion_error| "protocol minor exceeds u16")?,
        u16::try_from(version.patch).map_err(|_conversion_error| "protocol patch exceeds u16")?,
    ])
}

/// A version the dongle did not report is `[0, 0, 0]`.
fn firmware_version_triplet(version: Option<proto::FirmwareVersion>) -> [u32; 3] {
    version.map_or([0; 3], |version| {
        [version.major, version.minor, version.patch]
    })
}

/// Reject a malformed list rather than silently hiding a connected device.
fn addresses(raw: &[Vec<u8>]) -> Result<Vec<[u8; 6]>, &'static str> {
    raw.iter()
        .map(|bytes| {
            <[u8; 6]>::try_from(bytes.as_slice())
                .map_err(|_conversion_error| "dongle hardware address must contain exactly 6 bytes")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(major: u32, minor: u32, patch: u32) -> proto::FirmwareVersion {
        proto::FirmwareVersion {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn converts_versions_and_defaults_missing_ones_to_zero() {
        let info = DongleInfo::try_from(proto::Dongle {
            id: "dongle-1".to_owned(),
            firmware_version: Some(version(1, 2, 3)),
            protocol_version: Some(version(4, 5, 6)),
            control_protocol_version: None,
            connected_devices: vec![],
            bonded_devices: vec![],
        })
        .unwrap();
        assert_eq!(info.id, "dongle-1");
        assert_eq!(info.firmware_version, [1, 2, 3]);
        assert_eq!(info.protocol_version, [4, 5, 6]);
        assert_eq!(info.control_protocol_version, [0, 0, 0]);
    }

    #[test]
    fn malformed_addresses_are_refused() {
        DongleInfo::try_from(proto::Dongle {
            connected_devices: vec![vec![2; 5]],
            ..proto::Dongle::default()
        })
        .expect_err("an address of 5 bytes must be refused");
        let info = DongleInfo::try_from(proto::Dongle {
            firmware_version: Some(version(65_536, 0, 0)),
            ..proto::Dongle::default()
        })
        .unwrap();
        assert_eq!(info.firmware_version, [65_536, 0, 0]);
    }
}
