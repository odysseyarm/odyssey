use crate::hash::Sha256;
use crate::keys::PublicKey;
use crate::message::{Component, DOMAIN, MESSAGE_LEN, message};
use crate::verify::{SIGNATURE_LEN, verify, verify_with_keys};

/// Bytes in the signed metadata header, including reserved zero byte.
pub const DFU_HEADER_LEN: usize = MESSAGE_LEN + SIGNATURE_LEN + 1;
const DOMAIN_LEN: usize = DOMAIN.len();
const _: () = assert!(DFU_HEADER_LEN == 132 && DFU_HEADER_LEN.is_multiple_of(4));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DfuError {
    Header,
    Component,
    Length,
    Signature,
    Overflow,
    Truncated,
    ExtraPayload,
    Hash,
}

impl core::fmt::Display for DfuError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Header => "invalid signed DFU header",
            Self::Component => "DFU component is not allowed",
            Self::Length => "DFU image is smaller than its vector table",
            Self::Signature => "DFU signature is not trusted",
            Self::Overflow => "DFU image length overflowed",
            Self::Truncated => "DFU image is incomplete",
            Self::ExtraPayload => "DFU payload exceeds its signed length",
            Self::Hash => "DFU image hash does not match its signature",
        })
    }
}

impl core::error::Error for DfuError {}

#[must_use]
pub fn dfu_header(
    component: Component,
    size: u32,
    version: [u32; 3],
    sha256: &[u8; 32],
    signature: &[u8; SIGNATURE_LEN],
) -> [u8; DFU_HEADER_LEN] {
    let signed = message(component, size, version, sha256);
    let mut header = [0; DFU_HEADER_LEN];
    header[..MESSAGE_LEN].copy_from_slice(&signed);
    header[MESSAGE_LEN..MESSAGE_LEN + SIGNATURE_LEN].copy_from_slice(signature);
    header
}

#[derive(Debug)]
pub struct DfuVerifier {
    expected_size: u32,
    received: u32,
    expected_hash: [u8; 32],
    hash: Sha256,
}

impl DfuVerifier {
    /// Starts verifying a download from its signed header.
    ///
    /// # Errors
    /// Returns [`DfuError`] when the header is malformed, names another component or has a bad signature.
    pub fn new(component: Component, header: &[u8]) -> Result<Self, DfuError> {
        Self::new_inner(component, header, verify)
    }

    /// Starts verifying a download with caller-supplied public keys.
    ///
    /// # Errors
    /// Returns [`DfuError`] when the header is malformed, names another component or has a bad signature.
    pub fn new_with_keys(
        component: Component,
        header: &[u8],
        keys: &[PublicKey],
    ) -> Result<Self, DfuError> {
        Self::new_inner(component, header, |message, signature| {
            verify_with_keys(keys, message, signature)
        })
    }

    fn new_inner(
        component: Component,
        header: &[u8],
        verify_signature: impl FnOnce(&[u8; MESSAGE_LEN], &[u8; SIGNATURE_LEN]) -> bool,
    ) -> Result<Self, DfuError> {
        let header: &[u8; DFU_HEADER_LEN] = header.try_into().map_err(|_| DfuError::Header)?;
        if &header[..DOMAIN_LEN] != DOMAIN || header[DFU_HEADER_LEN - 1] != 0 {
            return Err(DfuError::Header);
        }
        let id = header[DOMAIN_LEN];
        if id != component as u8 {
            return Err(DfuError::Component);
        }
        let size = u32::from_le_bytes(header[19..23].try_into().map_err(|_| DfuError::Header)?);
        if size < 8 {
            return Err(DfuError::Length);
        }
        let expected_hash: [u8; 32] = header[35..67].try_into().map_err(|_| DfuError::Header)?;
        let signature: &[u8; SIGNATURE_LEN] = header[MESSAGE_LEN..MESSAGE_LEN + SIGNATURE_LEN]
            .try_into()
            .map_err(|_| DfuError::Header)?;
        let signed: &[u8; MESSAGE_LEN] = header[..MESSAGE_LEN]
            .try_into()
            .map_err(|_| DfuError::Header)?;
        if !verify_signature(signed, signature) {
            return Err(DfuError::Signature);
        }
        Ok(Self {
            expected_size: size,
            received: 0,
            expected_hash,
            hash: Sha256::new(),
        })
    }

    #[must_use]
    pub const fn image_size(&self) -> u32 {
        self.expected_size
    }

    /// Feeds the next payload chunk into the image hash.
    ///
    /// # Errors
    /// Returns [`DfuError`] when the chunk would exceed the signed image length.
    pub fn update(&mut self, payload: &[u8]) -> Result<(), DfuError> {
        let amount = u32::try_from(payload.len()).map_err(|_| DfuError::Overflow)?;
        let received = self
            .received
            .checked_add(amount)
            .ok_or(DfuError::Overflow)?;
        if received > self.expected_size {
            return Err(DfuError::ExtraPayload);
        }
        self.hash.update(payload);
        self.received = received;
        Ok(())
    }

    /// Checks that the whole image arrived and matches the signed hash.
    ///
    /// # Errors
    /// Returns [`DfuError`] when the image is short or its hash differs.
    pub fn finish(self) -> Result<(), DfuError> {
        if self.received != self.expected_size {
            return Err(DfuError::Truncated);
        }
        if self.hash.finalize() != self.expected_hash {
            return Err(DfuError::Hash);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{DFU_HEADER_LEN, DfuError, DfuVerifier, dfu_header};
    use crate::hash::Sha256;
    use crate::message::Component;

    const DEV_SEED: [u8; 32] = [
        0xda, 0x87, 0xce, 0x81, 0xe4, 0xa7, 0x02, 0xa2, 0x7b, 0x21, 0xed, 0xf4, 0xe4, 0x59, 0x4f,
        0x6f, 0xd2, 0x77, 0x18, 0x9e, 0xe3, 0x95, 0x8c, 0x64, 0x8e, 0x50, 0xec, 0x73, 0x35, 0xaa,
        0x4b, 0x04,
    ];

    fn fixture() -> [u8; DFU_HEADER_LEN] {
        let data = [0x5a; 8];
        let mut hash = Sha256::new();
        hash.update(&data);
        let digest = hash.finalize();
        let msg = crate::message::message(Component::Lite1App, 8, [1, 2, 3], &digest);
        let signature = salty::Keypair::from(&DEV_SEED).sign(&msg).to_bytes();
        dfu_header(Component::Lite1App, 8, [1, 2, 3], &digest, &signature)
    }

    #[test]
    fn rejects_untrusted_signature_before_payload() {
        let mut header = fixture();
        header[DFU_HEADER_LEN - 2] ^= 1;
        assert_eq!(
            DfuVerifier::new(Component::Lite1App, &header).err(),
            Some(DfuError::Signature)
        );
    }

    #[test]
    fn validates_reserved_domain_role_and_length() {
        let mut header = fixture();
        header[0] ^= 1;
        assert_eq!(
            DfuVerifier::new(Component::Lite1App, &header).err(),
            Some(DfuError::Header)
        );
        header = fixture();
        header[DFU_HEADER_LEN - 1] = 1;
        assert_eq!(
            DfuVerifier::new(Component::Lite1App, &header).err(),
            Some(DfuError::Header)
        );
        header = fixture();
        assert_eq!(
            DfuVerifier::new(Component::Lite1NetCore, &header).err(),
            Some(DfuError::Component)
        );
        assert_eq!(
            DfuVerifier::new(Component::Lite1App, &header[..DFU_HEADER_LEN - 1]).err(),
            Some(DfuError::Header)
        );
    }

    #[cfg(feature = "dev-signing-key")]
    #[test]
    fn rejects_truncated_extra_and_hash_mismatch() {
        let header = fixture();
        let verifier = DfuVerifier::new(Component::Lite1App, &header).unwrap();
        assert_eq!(verifier.finish().err(), Some(DfuError::Truncated));
        let mut verifier = DfuVerifier::new(Component::Lite1App, &header).unwrap();
        assert_eq!(verifier.update(&[0; 9]), Err(DfuError::ExtraPayload));
        let mut verifier = DfuVerifier::new(Component::Lite1App, &header).unwrap();
        verifier.update(&[0; 8]).unwrap();
        assert_eq!(verifier.finish().err(), Some(DfuError::Hash));
    }

    #[test]
    fn rejects_zero_length_images() {
        let mut header = fixture();
        header[19..23].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            DfuVerifier::new(Component::Lite1App, &header).err(),
            Some(DfuError::Length)
        );
    }
    #[cfg(feature = "dev-signing-key")]
    #[test]
    fn accepts_valid_dev_signed_image() {
        let header = fixture();
        let mut verifier = DfuVerifier::new(Component::Lite1App, &header).unwrap();
        assert_eq!(verifier.image_size(), 8);
        verifier.update(&[0x5a; 8]).unwrap();
        assert_eq!(verifier.finish(), Ok(()));
    }

    #[cfg(not(feature = "dev-signing-key"))]
    #[test]
    fn production_build_rejects_public_dev_key() {
        assert_eq!(
            DfuVerifier::new(Component::Lite1App, &fixture()).err(),
            Some(DfuError::Signature),
        );
    }
}
