use crate::keys::{EMPTY_KEY, PublicKey, trusted_keys};
use crate::message::MESSAGE_LEN;

/// Length of an Ed25519 signature.
///
/// Since: Unreleased.
pub const SIGNATURE_LEN: usize = 64;

/// Whether any trusted, non-empty key signed `message`.
///
/// Since: Unreleased.
#[must_use]
pub fn verify(message: &[u8; MESSAGE_LEN], signature: &[u8; SIGNATURE_LEN]) -> bool {
    verify_with(trusted_keys(), message, signature)
}

/// Whether a production release key signed `message`, excluding development keys.
///
/// Since: Unreleased.
#[must_use]
pub fn verify_production(message: &[u8; MESSAGE_LEN], signature: &[u8; SIGNATURE_LEN]) -> bool {
    verify_with(crate::keys::RELEASE_KEYS.iter(), message, signature)
}

fn verify_with<'a>(
    keys: impl IntoIterator<Item = &'a PublicKey>,
    message: &[u8; MESSAGE_LEN],
    signature: &[u8; SIGNATURE_LEN],
) -> bool {
    let signature = salty::Signature::from(signature);
    for key in keys {
        if *key == EMPTY_KEY {
            continue;
        }
        if let Ok(key) = salty::PublicKey::try_from(key)
            && key.verify(message, &signature).is_ok()
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use salty::Keypair;

    use super::{MESSAGE_LEN, SIGNATURE_LEN, verify, verify_with};
    use crate::keys::{DEV_KEY, EMPTY_KEY, PublicKey, RELEASE_KEYS, trusted_keys};
    use crate::message::{Component, message};

    /// The development seed. Public on purpose: it only signs dev builds.
    /// `crates/xtask/src/firmware/signing.rs` carries the same value.
    const DEV_SEED: [u8; 32] = [
        0xda, 0x87, 0xce, 0x81, 0xe4, 0xa7, 0x02, 0xa2, 0x7b, 0x21, 0xed, 0xf4, 0xe4, 0x59, 0x4f,
        0x6f, 0xd2, 0x77, 0x18, 0x9e, 0xe3, 0x95, 0x8c, 0x64, 0x8e, 0x50, 0xec, 0x73, 0x35, 0xaa,
        0x4b, 0x04,
    ];

    fn sign(message: &[u8; MESSAGE_LEN]) -> [u8; SIGNATURE_LEN] {
        Keypair::from(&DEV_SEED).sign(message).to_bytes()
    }

    fn base() -> [u8; MESSAGE_LEN] {
        message(Component::Lite1App, 1000, [1, 2, 3], &[7; 32])
    }

    #[test]
    fn dev_key_is_the_public_half_of_the_dev_seed() {
        assert_eq!(Keypair::from(&DEV_SEED).public.to_bytes(), DEV_KEY);
    }

    #[test]
    fn dev_signature_roundtrips() {
        let message = base();
        assert!(verify_with(&[DEV_KEY], &message, &sign(&message)));
    }

    #[test]
    fn changed_component_size_version_or_hash_fails() {
        let signature = sign(&base());
        let tampered = [
            message(Component::Lite1NetCore, 1000, [1, 2, 3], &[7; 32]),
            message(Component::Lite1App, 1001, [1, 2, 3], &[7; 32]),
            message(Component::Lite1App, 1000, [1, 2, 4], &[7; 32]),
            message(Component::Lite1App, 1000, [1, 2, 3], &[8; 32]),
        ];
        for message in tampered {
            assert!(
                !verify_with(&[DEV_KEY], &message, &signature),
                "{message:?}"
            );
        }
    }

    #[test]
    fn flipped_signature_bit_fails() {
        let message = base();
        for bit in [0, 255, 256, 511] {
            let mut signature = sign(&message);
            signature[bit / 8] ^= 1 << (bit % 8);
            assert!(!verify_with(&[DEV_KEY], &message, &signature), "bit {bit}");
        }
    }

    #[test]
    fn empty_slot_never_verifies() {
        let message = base();
        for signature in [[0; SIGNATURE_LEN], sign(&message)] {
            assert!(!verify_with(&[EMPTY_KEY], &message, &signature));
        }
    }

    #[test]
    fn any_one_key_among_many_is_enough() {
        let message = base();
        let keys: [PublicKey; 3] = [EMPTY_KEY, [9; 32], DEV_KEY];
        assert!(verify_with(&keys, &message, &sign(&message)));
    }

    #[test]
    fn production_keys_are_distinct_and_not_the_dev_key() {
        let used = |key: &&PublicKey| **key != EMPTY_KEY;
        assert!(RELEASE_KEYS.iter().any(|key| used(&key)));
        assert!(RELEASE_KEYS.iter().filter(used).all(|key| *key != DEV_KEY));
        for (i, key) in RELEASE_KEYS.iter().enumerate().filter(|(_, key)| used(key)) {
            assert!(RELEASE_KEYS[..i].iter().all(|other| other != key));
        }
    }

    #[test]
    fn production_verifier_rejects_the_dev_key_even_in_dev_builds() {
        let message = base();
        assert!(!super::verify_production(&message, &sign(&message)));
    }

    #[cfg(feature = "dev-signing-key")]
    #[test]
    fn dev_feature_trusts_the_dev_key() {
        let message = base();
        assert!(trusted_keys().any(|key| *key == DEV_KEY));
        assert!(verify(&message, &sign(&message)));
    }

    #[cfg(not(feature = "dev-signing-key"))]
    #[test]
    fn release_build_rejects_the_dev_key() {
        let message = base();
        assert!(trusted_keys().all(|key| *key != DEV_KEY));
        assert!(!verify(&message, &sign(&message)));
    }
}
