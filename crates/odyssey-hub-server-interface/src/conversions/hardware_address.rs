//! Helpers for the 6-byte hardware addresses that protobuf carries either as
//! `bytes` (copied leniently) or as the low 48 bits of a `u64` map key.

/// Length of a device/dongle hardware address.
pub(super) const ADDRESS_LEN: usize = 6;

/// Copies up to six bytes, zero-padding short input and ignoring the excess of
/// long input, so a malformed peer cannot make the conversion fail.
pub(super) fn address_from_bytes(bytes: &[u8]) -> [u8; ADDRESS_LEN] {
    let mut address = [0u8; ADDRESS_LEN];
    let len = bytes.len().min(ADDRESS_LEN);
    address[..len].copy_from_slice(&bytes[..len]);
    address
}

/// Packs an address into a `u64` map key (little endian, top 16 bits zero).
pub(super) fn address_to_key(address: [u8; ADDRESS_LEN]) -> u64 {
    let [a, b, c, d, e, f] = address;
    u64::from_le_bytes([a, b, c, d, e, f, 0, 0])
}

/// Inverse of [`address_to_key`]; the top 16 bits of `key` are dropped.
pub(super) fn address_from_key(key: u64) -> [u8; ADDRESS_LEN] {
    let [a, b, c, d, e, f, _, _] = key.to_le_bytes();
    [a, b, c, d, e, f]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_round_trips() {
        let address = [0x01, 0x02, 0x03, 0x04, 0x05, 0xff];
        assert_eq!(address_from_key(address_to_key(address)), address);
        assert_eq!(address_to_key(address), 0x0000_ff05_0403_0201);
    }

    #[test]
    fn key_drops_top_bits() {
        assert_eq!(address_from_key(u64::MAX), [0xff; ADDRESS_LEN]);
    }

    #[test]
    fn bytes_are_padded_or_truncated() {
        assert_eq!(address_from_bytes(&[]), [0; ADDRESS_LEN]);
        assert_eq!(address_from_bytes(&[1, 2]), [1, 2, 0, 0, 0, 0]);
        assert_eq!(
            address_from_bytes(&[1, 2, 3, 4, 5, 6, 7]),
            [1, 2, 3, 4, 5, 6]
        );
    }
}
