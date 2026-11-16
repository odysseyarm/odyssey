//! Marker-control-board GATT protocol. Accessory registration is not BLE bonding.

/// UUID bytes in canonical (big-endian) order. Reverse for BLE advertising data.
///
/// Since: Odyssey Desktop 0.7.4.
pub const SERVICE_UUID: [u8; 16] = 0x9b7a0001_4f4e_4f44_5953_53455941524du128.to_be_bytes();
/// Since: Odyssey Desktop 0.7.4.
pub const DEMAND_UUID: [u8; 16] = 0x9b7a0002_4f4e_4f44_5953_53455941524du128.to_be_bytes();
/// Since: Odyssey Desktop 0.7.4.
pub const MODE_UUID: [u8; 16] = 0x9b7a0003_4f4e_4f44_5953_53455941524du128.to_be_bytes();
/// Since: Unreleased.
pub const OTA_UUID: [u8; 16] = 0x9b7a0004_4f4e_4f44_5953_53455941524du128.to_be_bytes();
/// Since: Odyssey Desktop 0.7.4.
pub const LEASE_MILLIS: u64 = 6000;
/// Since: Odyssey Desktop 0.7.4.
pub const RENEW_MILLIS: u64 = 2000;
/// Since: Odyssey Desktop 0.7.4.
pub const ENROLLMENT_SECONDS: u8 = 30;
/// Since: Odyssey Desktop 0.7.4.
pub const PAIRING_STATE_VERSION: u8 = 1;

/// Advisory physical pairing-window state from marker `ServiceData128`.
///
/// Since: Odyssey Desktop 0.7.4.
pub fn parse_pairing_state(data: &[u8]) -> Option<u8> {
    match data {
        [PAIRING_STATE_VERSION, seconds] if *seconds <= ENROLLMENT_SECONDS => Some(*seconds),
        _ => None,
    }
}

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum Mode {
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire: unversioned BLE Mode characteristic.
    #[default]
    Auto = 0,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire: unversioned BLE Mode characteristic.
    Off = 1,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire: unversioned BLE Mode characteristic.
    On = 2,
}

/// Since: Odyssey Desktop 0.7.4.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteError {
    /// Since: Odyssey Desktop 0.7.4.
    Length,
    /// Since: Odyssey Desktop 0.7.4.
    Value,
}

impl Mode {
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// # Errors
    /// Returns an error if input does not contain exactly one recognized mode byte.
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
///
/// Since: Odyssey Desktop 0.7.4.
///
/// # Errors
/// Returns an error if input does not contain exactly one recognized demand byte.
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
