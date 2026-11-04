use odyssey_hub_server_interface as proto;

/// Dongle metadata returned by the dongle list subscription.
/// Conversion rejects malformed hardware addresses and overflowing version fields.
#[derive(Clone, Debug, PartialEq)]
pub struct DongleInfo {
    pub id: String,
    pub protocol_version: [u16; 3],         // mux endpoint protocol
    pub control_protocol_version: [u16; 3], // control endpoint protocol
    pub firmware_version: [u16; 3],
    pub connected_devices: Vec<[u8; 6]>,
    pub bonded_devices: Vec<[u8; 6]>,
}

impl TryFrom<proto::Dongle> for DongleInfo {
    type Error = &'static str;

    fn try_from(dongle: proto::Dongle) -> Result<Self, Self::Error> {
        Ok(Self {
            id: dongle.id,
            protocol_version: version_triplet(dongle.protocol_version)?,
            control_protocol_version: version_triplet(dongle.control_protocol_version)?,
            firmware_version: version_triplet(dongle.firmware_version)?,
            connected_devices: addresses(&dongle.connected_devices)?,
            bonded_devices: addresses(&dongle.bonded_devices)?,
        })
    }
}

/// A version the dongle did not report is `[0, 0, 0]`.
fn version_triplet(version: Option<proto::FirmwareVersion>) -> Result<[u16; 3], &'static str> {
    let Some(version) = version else {
        return Ok([0; 3]);
    };
    Ok([
        u16::try_from(version.major)
            .map_err(|_conversion_error| "dongle version major exceeds u16")?,
        u16::try_from(version.minor)
            .map_err(|_conversion_error| "dongle version minor exceeds u16")?,
        u16::try_from(version.patch)
            .map_err(|_conversion_error| "dongle version patch exceeds u16")?,
    ])
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

    fn version(major: u32, minor: u32, patch: u32) -> Option<proto::FirmwareVersion> {
        Some(proto::FirmwareVersion {
            major,
            minor,
            patch,
        })
    }

    #[test]
    fn converts_versions_and_defaults_missing_ones_to_zero() {
        let info = DongleInfo::try_from(proto::Dongle {
            id: "dongle-1".to_owned(),
            firmware_version: version(1, 2, 3),
            protocol_version: version(4, 5, 6),
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
    fn malformed_addresses_and_versions_are_refused() {
        assert!(
            DongleInfo::try_from(proto::Dongle {
                connected_devices: vec![vec![2; 5]],
                ..proto::Dongle::default()
            })
            .is_err()
        );
        assert!(
            DongleInfo::try_from(proto::Dongle {
                firmware_version: version(65536, 0, 0),
                ..proto::Dongle::default()
            })
            .is_err()
        );
    }
}
