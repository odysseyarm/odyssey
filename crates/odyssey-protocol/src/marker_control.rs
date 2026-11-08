//! Marker-control-board GATT protocol. Accessory registration is not BLE bonding.

/// UUID bytes in canonical (big-endian) order; reverse for BLE advertising data.
pub const SERVICE_UUID: [u8; 16] = 0x9b7a0001_4f4e_4f44_5953_53455941524du128.to_be_bytes();
pub const DEMAND_UUID: [u8; 16] = 0x9b7a0002_4f4e_4f44_5953_53455941524du128.to_be_bytes();
pub const MODE_UUID: [u8; 16] = 0x9b7a0003_4f4e_4f44_5953_53455941524du128.to_be_bytes();
pub const LEASE_MILLIS: u64 = 6000;
pub const RENEW_MILLIS: u64 = 2000;
pub const ENROLLMENT_SECONDS: u8 = 30;
pub const PAIRING_STATE_VERSION: u8 = 1;

/// Advisory physical pairing-window state from marker ServiceData128.
pub fn parse_pairing_state(data: &[u8]) -> Option<u8> {
    match data {
        [PAIRING_STATE_VERSION, seconds] if *seconds <= ENROLLMENT_SECONDS => Some(*seconds),
        _ => None,
    }
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum Mode {
    #[default]
    Auto = 0,
    Off = 1,
    On = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteError {
    Length,
    Value,
}

impl Mode {
    pub fn parse(data: &[u8]) -> Result<Self, WriteError> {
        match data {
            [0] => Ok(Self::Auto),
            [1] => Ok(Self::Off),
            [2] => Ok(Self::On),
            [_] => Err(WriteError::Value),
            _ => Err(WriteError::Length),
        }
    }
}

/// Demand accepts acknowledged writes only: `[0]` clears, `[1]` renews lease.
pub fn parse_demand(data: &[u8]) -> Result<bool, WriteError> {
    match data {
        [0] => Ok(false),
        [1] => Ok(true),
        [_] => Err(WriteError::Value),
        _ => Err(WriteError::Length),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_state_requires_supported_version_exact_length_and_bounded_seconds() {
        for seconds in 0..=ENROLLMENT_SECONDS {
            assert_eq!(
                parse_pairing_state(&[PAIRING_STATE_VERSION, seconds]),
                Some(seconds)
            );
        }
        for data in [
            &[][..],
            &[PAIRING_STATE_VERSION][..],
            &[PAIRING_STATE_VERSION, 1, 0][..],
            &[0, 1][..],
            &[PAIRING_STATE_VERSION + 1, 1][..],
            &[PAIRING_STATE_VERSION, ENROLLMENT_SECONDS + 1][..],
            &[PAIRING_STATE_VERSION, u8::MAX][..],
        ] {
            assert_eq!(parse_pairing_state(data), None);
        }
    }
}
