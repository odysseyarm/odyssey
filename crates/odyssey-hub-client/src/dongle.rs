use odyssey_hub_server_interface as proto;

/// Dongle metadata returned by the dongle list subscription.
#[derive(Clone, Debug, PartialEq)]
pub struct DongleInfo {
    pub id: String,
    pub protocol_version: [u16; 3],         // mux endpoint protocol
    pub control_protocol_version: [u16; 3], // control endpoint protocol
    pub firmware_version: [u16; 3],
    pub connected_devices: Vec<[u8; 6]>,
    pub bonded_devices: Vec<[u8; 6]>,
}

impl From<proto::Dongle> for DongleInfo {
    fn from(dongle: proto::Dongle) -> Self {
        Self {
            id: dongle.id,
            protocol_version: version_triplet(dongle.protocol_version),
            control_protocol_version: version_triplet(dongle.control_protocol_version),
            firmware_version: version_triplet(dongle.firmware_version),
            connected_devices: addresses(&dongle.connected_devices),
            bonded_devices: addresses(&dongle.bonded_devices),
        }
    }
}

/// A version the dongle did not report is `[0, 0, 0]`.
fn version_triplet(version: Option<proto::FirmwareVersion>) -> [u16; 3] {
    version.map_or([0, 0, 0], |v| {
        [v.major as u16, v.minor as u16, v.patch as u16]
    })
}

/// Entries that are not exactly six bytes are dropped rather than padded: a
/// wrong-sized address would name a device that does not exist.
fn addresses(raw: &[Vec<u8>]) -> Vec<[u8; 6]> {
    raw.iter()
        .filter_map(|bytes| <[u8; 6]>::try_from(bytes.as_slice()).ok())
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
        let info = DongleInfo::from(proto::Dongle {
            id: "dongle-1".to_owned(),
            firmware_version: version(1, 2, 3),
            protocol_version: version(4, 5, 6),
            control_protocol_version: None,
            connected_devices: vec![],
            bonded_devices: vec![],
        });
        assert_eq!(info.id, "dongle-1");
        assert_eq!(info.firmware_version, [1, 2, 3]);
        assert_eq!(info.protocol_version, [4, 5, 6]);
        assert_eq!(info.control_protocol_version, [0, 0, 0]);
    }

    #[test]
    fn drops_addresses_that_are_not_six_bytes() {
        let info = DongleInfo::from(proto::Dongle {
            connected_devices: vec![vec![1; 6], vec![2; 5], vec![3; 7]],
            bonded_devices: vec![vec![], vec![9; 6]],
            ..proto::Dongle::default()
        });
        assert_eq!(info.connected_devices, vec![[1; 6]]);
        assert_eq!(info.bonded_devices, vec![[9; 6]]);
    }
}
