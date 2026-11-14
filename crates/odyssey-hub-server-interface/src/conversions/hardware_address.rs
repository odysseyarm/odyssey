//! Checked helpers for 6-byte hardware addresses carried as protobuf `bytes`
//! or as the low 48 bits of a `u64` map key.

/// Length of a device/dongle hardware address.
pub(super) const ADDRESS_LEN: usize = 6;

/// Reject malformed lengths rather than aliasing another device's address.
pub(super) fn address_from_bytes(bytes: &[u8]) -> Result<[u8; ADDRESS_LEN], &'static str> {
    bytes
        .try_into()
        .map_err(|_conversion_error| "hardware address must contain exactly 6 bytes")
}

/// Packs an address into a `u64` map key (little endian, top 16 bits zero).
pub(super) fn address_to_key(address: [u8; ADDRESS_LEN]) -> u64 {
    let [a, b, c, d, e, f] = address;
    u64::from_le_bytes([a, b, c, d, e, f, 0, 0])
}

/// Inverse of [`address_to_key`]. High bits must not alias a real map entry.
pub(super) fn address_from_key(key: u64) -> Result<[u8; ADDRESS_LEN], &'static str> {
    if key >> 48 != 0 {
        return Err("hardware address map key exceeds 48 bits");
    }
    let [a, b, c, d, e, f, _, _] = key.to_le_bytes();
    Ok([a, b, c, d, e, f])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_round_trips() {
        let address = [0x01, 0x02, 0x03, 0x04, 0x05, 0xff];
        assert_eq!(address_from_key(address_to_key(address)).unwrap(), address);
        assert_eq!(address_to_key(address), 0x0000_ff05_0403_0201);
    }

    #[test]
    fn malformed_map_keys_are_not_aliased_to_valid_addresses() {
        address_from_key(u64::MAX).expect_err("address key exceeding 48 bits must be rejected");
    }

    #[test]
    fn malformed_byte_lengths_are_refused() {
        address_from_bytes(&[1; 5]).expect_err("short address must be rejected");
        address_from_bytes(&[1; 7]).expect_err("long address must be rejected");
        assert_eq!(address_from_bytes(&[1; 6]).unwrap(), [1; 6]);
    }
}
