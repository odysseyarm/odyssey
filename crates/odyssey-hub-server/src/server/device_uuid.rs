use odyssey_hub_server_interface::Device;

use super::rpc_error::RpcError;

const UUID_LEN: usize = 6;

/// A request must identify exactly one six-byte UUID; padding or truncating can
/// accidentally address another connected unit.
pub(super) fn uuid_from_bytes(bytes: &[u8]) -> Result<[u8; UUID_LEN], RpcError> {
    bytes.try_into().map_err(|_conversion_error| {
        RpcError::InvalidField("device.uuid length (expected 6 bytes)")
    })
}

/// The uuid of the device a request names; an unset `device` is the caller's error.
pub(super) fn requested_uuid(device: Option<Device>) -> Result<[u8; UUID_LEN], RpcError> {
    let device = device.ok_or(RpcError::MissingField("device"))?;
    uuid_from_bytes(&device.uuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_uuid_lengths_are_refused_instead_of_aliasing_another_device() {
        assert!(uuid_from_bytes(&[1; 5]).is_err());
        assert!(uuid_from_bytes(&[1; 7]).is_err());
        assert_eq!(uuid_from_bytes(&[1; 6]).unwrap(), [1; 6]);
    }

    #[test]
    fn unset_device_is_a_missing_field() {
        assert!(matches!(
            requested_uuid(None),
            Err(RpcError::MissingField("device"))
        ));
    }
}
