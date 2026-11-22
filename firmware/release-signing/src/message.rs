pub(crate) const DOMAIN: &[u8; 18] = b"ODYSSEY-FW-SIG-V1\0";

/// Length of [`message`]: domain tag, component, size, version, image hash.
///
/// Since: Unreleased.
pub const MESSAGE_LEN: usize = DOMAIN.len() + 1 + 4 + 12 + 32;

/// The image a signature is for. The id is part of the signed message, so a
/// signed image cannot be moved to another product or role.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Component {
    Lite1App = 0,
    Lite1NetCore = 1,
    LegacyAtsliteApp = 2,
    LegacyAtsliteNetCore = 3,
    MarkerApp = 4,
    DongleApp = 5,
    Lite1Bootloader = 6,
    LegacyAtsliteBootloader = 7,
    Lite1TrustList = 8,
    Lite1SecureApp = 9,
    Lite1SecureNetCore = 10,
    Lite1SecureBootloader = 11,
}

/// Bytes covered by a release signature.
///
/// `version` is the application `SemVer` as `[major, minor, patch]`, or
/// `[release, 0, 0]` for a net core. Binding it stops an old signed image from
/// passing as a new one. Integers are little-endian.
///
/// Since: Unreleased.
#[must_use]
pub fn message(
    component: Component,
    size: u32,
    version: [u32; 3],
    sha256: &[u8; 32],
) -> [u8; MESSAGE_LEN] {
    let mut out = [0; MESSAGE_LEN];
    out[..18].copy_from_slice(DOMAIN);
    out[18] = component as u8;
    out[19..23].copy_from_slice(&size.to_le_bytes());
    for (i, part) in version.into_iter().enumerate() {
        out[23 + 4 * i..27 + 4 * i].copy_from_slice(&part.to_le_bytes());
    }
    out[35..].copy_from_slice(sha256);
    out
}

#[cfg(test)]
mod tests {
    use super::{Component, MESSAGE_LEN, message};

    #[test]
    fn layout_is_frozen() {
        let hash: [u8; 32] = core::array::from_fn(|i| 0xa0 + i as u8);
        let got = message(Component::MarkerApp, 0x0102_0304, [1, 2, 3], &hash);
        let mut want = b"ODYSSEY-FW-SIG-V1\0".to_vec();
        want.push(4);
        want.extend([0x04, 0x03, 0x02, 0x01]);
        want.extend([1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]);
        want.extend(hash);
        assert_eq!(MESSAGE_LEN, 67);
        assert_eq!(got.as_slice(), want.as_slice());
    }

    #[test]
    fn component_ids_are_frozen() {
        let ids = [
            (Component::Lite1App, 0),
            (Component::Lite1NetCore, 1),
            (Component::LegacyAtsliteApp, 2),
            (Component::LegacyAtsliteNetCore, 3),
            (Component::MarkerApp, 4),
            (Component::DongleApp, 5),
            (Component::Lite1Bootloader, 6),
            (Component::LegacyAtsliteBootloader, 7),
            (Component::Lite1TrustList, 8),
            (Component::Lite1SecureApp, 9),
            (Component::Lite1SecureNetCore, 10),
            (Component::Lite1SecureBootloader, 11),
        ];
        for (component, id) in ids {
            assert_eq!(
                message(component, 0, [0; 3], &[0; 32])[18],
                id,
                "{component:?}"
            );
        }
    }
}
