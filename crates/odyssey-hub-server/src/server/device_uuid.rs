use odyssey_hub_server_interface::Device;

use super::rpc_error::RpcError;

const UUID_LEN: usize = 6;

/// A device uuid from the bytes of a request: shorter input is zero padded and
/// longer input is cut off.
pub(super) fn uuid_from_bytes(bytes: &[u8]) -> [u8; UUID_LEN] {
    let mut uuid = [0; UUID_LEN];
    let len = bytes.len().min(UUID_LEN);
    uuid[..len].copy_from_slice(&bytes[..len]);
    uuid
}

/// The uuid of the device a request names; an unset `device` is the caller's error.
pub(super) fn requested_uuid(device: Option<Device>) -> Result<[u8; UUID_LEN], RpcError> {
    let device = device.ok_or(RpcError::MissingField("device"))?;
    Ok(uuid_from_bytes(&device.uuid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_uuids_are_zero_padded_and_long_ones_cut() {
        assert_eq!(uuid_from_bytes(&[]), [0; 6]);
        assert_eq!(uuid_from_bytes(&[1, 2]), [1, 2, 0, 0, 0, 0]);
        assert_eq!(uuid_from_bytes(&[1, 2, 3, 4, 5, 6]), [1, 2, 3, 4, 5, 6]);
        assert_eq!(
            uuid_from_bytes(&[1, 2, 3, 4, 5, 6, 7, 8]),
            [1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn unset_device_is_a_missing_field() {
        assert!(matches!(
            requested_uuid(None),
            Err(RpcError::MissingField("device"))
        ));
    }
}
