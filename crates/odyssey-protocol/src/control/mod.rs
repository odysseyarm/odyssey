/// Since: Odyssey Desktop 0.6.0.
pub mod device;
/// Since: Odyssey Desktop 0.6.0.
pub mod usb_mux;

use odyssey_protocol_macros::protocol_type;

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "minicbor", cbor(map))]
#[derive(Clone, Copy, Debug, Default)]
pub struct BondEntry {
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub bd_addr: [u8; 6],
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub ltk: [u8; 16],
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(2))]
    pub security_level: u8,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(3))]
    pub is_bonded: bool,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(4))]
    pub irk: Option<[u8; 16]>,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
pub struct BondedDevice {
    /// Since: Odyssey Desktop 0.6.0.
    pub uuid: [u8; 6],
    /// Since: Odyssey Desktop 0.6.0.
    pub name: crate::DeviceName,
}

/// Most BLE bonds a tracker stores. Pairing another host while all are in use fails
/// with [`device::PairingError::BondsFull`]; remove one first.
///
/// Since: Unreleased.
pub const MAX_BONDS: usize = 4;

/// BLE address a unit advertises with, derived from its uuid (least significant byte first).
///
/// The top two bits are forced to `11`: the Linux kernel only stores bonds for a random
/// address that is static, so a unit whose uuid ends in another pattern could not bond with
/// `BlueZ`.
///
/// Since: Unreleased.
#[must_use]
pub const fn ble_address(uuid: [u8; 6]) -> [u8; 6] {
    /// Top two bits of a static random address (Core spec Vol 6 Part B 1.3.2.1).
    const STATIC_RANDOM: u8 = 0xC0;
    let mut address = uuid;
    address[5] |= STATIC_RANDOM;
    address
}

/// BLE address of the unit with `uuid` whose firmware reports device protocol `protocol`:
/// [`ble_address`] from 0.2.2, the bare uuid before.
///
/// Since: Unreleased.
#[must_use]
pub fn ble_address_for(uuid: [u8; 6], protocol: [u16; 3]) -> [u8; 6] {
    if protocol >= [0, 2, 2] {
        ble_address(uuid)
    } else {
        uuid
    }
}

/// One stored BLE bond, as listed by [`device::DeviceMsg::ListBonds`].
///
/// Since: Unreleased.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BondSlot {
    /// Identity address of the bonded host.
    ///
    /// Since: Unreleased.
    pub bd_addr: [u8; 6],
    /// Whether this host holds the unit's BLE link right now.
    ///
    /// Since: Unreleased.
    pub connected: bool,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub enum AddBondError {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Full,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Failed,
}

/// The result of an add-bond request. Serializes as the equivalent `Result`.
///
/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(
    feature = "serde",
    serde(from = "Result<(), AddBondError>", into = "Result<(), AddBondError>")
)]
#[derive(Clone, Copy, Debug)]
pub enum AddBondResult {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Ok(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Err(AddBondError),
}

impl From<Result<(), AddBondError>> for AddBondResult {
    fn from(value: Result<(), AddBondError>) -> Self {
        match value {
            Ok(()) => Self::Ok(),
            Err(e) => Self::Err(e),
        }
    }
}

impl From<AddBondResult> for Result<(), AddBondError> {
    fn from(value: AddBondResult) -> Self {
        match value {
            AddBondResult::Ok() => Ok(()),
            AddBondResult::Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ble_address, ble_address_for};

    #[test]
    fn ble_address_is_static_random_and_keeps_the_other_bits() {
        let uuid = [0x19, 0xe3, 0x54, 0x3e, 0x7b, 0x6e];
        let address = ble_address(uuid);
        assert_eq!(address[5] & 0xC0, 0xC0);
        assert_eq!(address[..5], uuid[..5]);
        assert_eq!(address[5], 0xEE);
        assert_eq!(ble_address(address), address);
    }

    #[test]
    fn older_firmware_advertises_with_its_bare_uuid() {
        let uuid = [0x19, 0xe3, 0x54, 0x3e, 0x7b, 0x6e];
        assert_eq!(ble_address_for(uuid, [0, 2, 1]), uuid);
        assert_eq!(ble_address_for(uuid, [0, 2, 2]), ble_address(uuid));
        assert_eq!(ble_address_for(uuid, [0, 3, 0]), ble_address(uuid));
    }
}
