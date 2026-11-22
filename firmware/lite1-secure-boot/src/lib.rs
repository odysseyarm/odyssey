#![no_std]

#[cfg(test)]
extern crate std;

use core::fmt;

use release_signing::{
    Component, DFU_HEADER_LEN, DfuError, DfuVerifier, PublicKey, RootSet, TrustList, TrustListError,
};

#[cfg(any(test, all(feature = "hardware", target_arch = "arm")))]
pub mod handoff;
#[cfg(all(feature = "hardware", target_arch = "arm"))]
pub mod hardware;
pub mod state;

/// Signed-slot prefix: signed DFU header followed by erased bytes.
pub const SLOT_HEADER_LEN: usize = 512;
const APP_STACK_RANGE: core::ops::RangeInclusive<u32> = 0x2000_2000..=0x2004_2000;
const NET_STACK_RANGE: core::ops::RangeInclusive<u32> = 0x2100_0000..=0x2101_0000;

/// Secure-boot verification failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Slot,
    Padding,
    AddressOverflow,
    Vector,
    Dfu(DfuError),
    Record,
    Policy(TrustListError),
    PolicyGeneration,
    AmbiguousPolicy,
    BootPolicy,
    Storage,
    Generation,
    Recovery,
    SelfTest,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Dfu(error) => Some(error),
            Self::Policy(error) => Some(error),
            _ => None,
        }
    }
}

const KAT_SHA256_ABC: [u8; 32] = [
    0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
    0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
];
/// Public key of the fixed test seed `[0x4b; 32]`; a test vector, never trusted.
const KAT_KEY: PublicKey = [
    0x7e, 0xa0, 0xe3, 0xbd, 0x52, 0xe2, 0x07, 0xc9, 0xd3, 0xb0, 0xeb, 0xa6, 0x5c, 0x07, 0x04, 0xe6,
    0x6f, 0xca, 0x2d, 0x8e, 0x16, 0x5a, 0x17, 0x52, 0x18, 0xb1, 0x74, 0xfc, 0x41, 0x60, 0xe4, 0x13,
];
const KAT_SIGNATURE: [u8; release_signing::SIGNATURE_LEN] = [
    0x01, 0x84, 0x23, 0x0f, 0xb8, 0xaf, 0xa1, 0xca, 0x3c, 0x6d, 0x4a, 0x96, 0x80, 0xd0, 0xf7, 0x69,
    0xa7, 0x84, 0x12, 0x7c, 0xd0, 0x2c, 0x9f, 0x99, 0x78, 0x4a, 0xf3, 0xac, 0x52, 0x74, 0x35, 0x71,
    0x17, 0x5f, 0xfb, 0x07, 0xe0, 0x0e, 0xfc, 0xd8, 0xa0, 0x51, 0x00, 0x62, 0xf4, 0x83, 0xef, 0x83,
    0x0c, 0xcb, 0x9a, 0xb2, 0xc2, 0xde, 0xce, 0x82, 0x9d, 0x38, 0x68, 0x85, 0x20, 0x4d, 0x41, 0x08,
];

/// FIPS 140-3 power-up known-answer tests of every algorithm boot relies on:
/// SHA-256, and Ed25519 verification including its internal SHA-512. Stage1 runs
/// this before verifying any image and refuses to boot when it fails. The
/// corrupted-signature case guards against a verifier that accepts everything.
pub fn self_test() -> Result<(), Error> {
    let mut hash = release_signing::Sha256::new();
    hash.update(b"abc");
    if hash.finalize() != KAT_SHA256_ABC {
        return Err(Error::SelfTest);
    }
    let message =
        release_signing::message(Component::Lite1SecureApp, 3, [0, 0, 0], &KAT_SHA256_ABC);
    if !release_signing::verify_with_keys(&[KAT_KEY], &message, &KAT_SIGNATURE) {
        return Err(Error::SelfTest);
    }
    let mut corrupted = KAT_SIGNATURE;
    corrupted[0] ^= 1;
    if release_signing::verify_with_keys(&[KAT_KEY], &message, &corrupted) {
        return Err(Error::SelfTest);
    }
    Ok(())
}

/// Verifies signed firmware slot bytes and its initial Cortex-M vectors.
pub fn verify_slot(
    slot: &[u8],
    slot_address: u32,
    component: Component,
    keys: &[PublicKey],
) -> Result<(), Error> {
    if slot.len() < SLOT_HEADER_LEN {
        return Err(Error::Slot);
    }
    if slot[DFU_HEADER_LEN..SLOT_HEADER_LEN]
        .iter()
        .any(|byte| *byte != 0xff)
    {
        return Err(Error::Padding);
    }
    let header = &slot[..DFU_HEADER_LEN];
    let mut verifier = DfuVerifier::new_with_keys(component, header, keys).map_err(Error::Dfu)?;
    let image_size = verifier.image_size();
    let image_len = usize::try_from(image_size).map_err(|_| Error::Slot)?;
    let payload_end = SLOT_HEADER_LEN.checked_add(image_len).ok_or(Error::Slot)?;
    let image = slot.get(SLOT_HEADER_LEN..payload_end).ok_or(Error::Slot)?;
    verifier.update(image).map_err(Error::Dfu)?;
    verifier.finish().map_err(Error::Dfu)?;

    let stack_pointer = u32::from_le_bytes(image[..4].try_into().map_err(|_| Error::Vector)?);
    let reset_vector = u32::from_le_bytes(image[4..8].try_into().map_err(|_| Error::Vector)?);
    let stack_range = match component {
        Component::Lite1SecureApp | Component::Lite1SecureBootloader => &APP_STACK_RANGE,
        Component::Lite1SecureNetCore => &NET_STACK_RANGE,
        _ => return Err(Error::Vector),
    };
    if !stack_range.contains(&stack_pointer) || stack_pointer & 7 != 0 || reset_vector & 1 == 0 {
        return Err(Error::Vector);
    }
    let image_start = slot_address
        .checked_add(u32::try_from(SLOT_HEADER_LEN).map_err(|_| Error::AddressOverflow)?)
        .ok_or(Error::AddressOverflow)?;
    let image_end = image_start
        .checked_add(image_size)
        .ok_or(Error::AddressOverflow)?;
    let reset_address = reset_vector & !1;
    if reset_address < image_start || reset_address >= image_end {
        return Err(Error::Vector);
    }
    Ok(())
}

/// Validates next signed policy generation.
pub fn next_policy(record: &[u8], generation: u16, roots: &RootSet) -> Result<TrustList, Error> {
    let next = generation.checked_add(1).ok_or(Error::Generation)?;
    verify_policy_record(record, next, roots)
}

fn verify_policy_record(
    record: &[u8],
    generation: u16,
    roots: &RootSet,
) -> Result<TrustList, Error> {
    let body = record.get(DFU_HEADER_LEN..).ok_or(Error::Record)?;
    let (list, entries) = TrustList::parse_record(body).map_err(Error::Policy)?;
    if list.generation() != generation {
        return Err(Error::PolicyGeneration);
    }
    if !list.authorized_by(roots, entries) {
        return Err(Error::Policy(TrustListError::Signatures));
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use release_signing::{
        Component, DfuError, MESSAGE_LEN, PublicKey, RootSet, Sha256, TrustList, dfu_header,
        message,
    };
    use salty::Keypair;

    use super::state::{self, STATE_LEN};
    use super::{Error, KAT_KEY, SLOT_HEADER_LEN, next_policy, self_test, verify_slot};
    fn key(seed: u8) -> PublicKey {
        Keypair::from(&[seed; 32]).public.to_bytes()
    }
    fn signed_header(
        component: Component,
        payload: &[u8],
        seed: u8,
    ) -> [u8; release_signing::DFU_HEADER_LEN] {
        let mut hash = Sha256::new();
        hash.update(payload);
        let digest = hash.finalize();
        let signed = message(component, payload.len() as u32, [1, 0, 0], &digest);
        let signature = Keypair::from(&[seed; 32]).sign(&signed).to_bytes();
        dfu_header(
            component,
            payload.len() as u32,
            [1, 0, 0],
            &digest,
            &signature,
        )
    }

    fn slot(seed: u8, reset: u32) -> std::vec::Vec<u8> {
        let mut image = [0u8; 16];
        image[..4].copy_from_slice(&0x2000_4000u32.to_le_bytes());
        image[4..8].copy_from_slice(&reset.to_le_bytes());
        let header = signed_header(Component::Lite1SecureApp, &image, seed);
        let mut slot = std::vec![0xff; SLOT_HEADER_LEN + image.len()];
        slot[..release_signing::DFU_HEADER_LEN].copy_from_slice(&header);
        slot[SLOT_HEADER_LEN..].copy_from_slice(&image);
        slot
    }

    #[test]
    fn verify_slot_checks_signature_role_padding_vectors_and_key_retirement() {
        let valid = slot(1, 0x1209);
        assert_eq!(
            verify_slot(&valid, 0x1000, Component::Lite1SecureApp, &[key(1)]),
            Ok(())
        );
        let mut tampered = valid.clone();
        tampered[MESSAGE_LEN] ^= 1;
        assert!(verify_slot(&tampered, 0x1000, Component::Lite1SecureApp, &[key(1)]).is_err());
        assert_eq!(
            verify_slot(&valid, 0x1000, Component::Lite1SecureApp, &[key(2)]),
            Err(Error::Dfu(DfuError::Signature)),
        );
        let wrong_role = signed_header(Component::Lite1App, &valid[SLOT_HEADER_LEN..], 1);
        let mut wrong = valid.clone();
        wrong[..release_signing::DFU_HEADER_LEN].copy_from_slice(&wrong_role);
        assert_eq!(
            verify_slot(&wrong, 0x1000, Component::Lite1SecureApp, &[key(1)]),
            Err(Error::Dfu(DfuError::Component)),
        );
        let mut padding = valid.clone();
        padding[release_signing::DFU_HEADER_LEN] = 0;
        assert_eq!(
            verify_slot(&padding, 0x1000, Component::Lite1SecureApp, &[key(1)]),
            Err(Error::Padding),
        );
        assert_eq!(
            verify_slot(
                &slot(1, 0x1409),
                0x1000,
                Component::Lite1SecureApp,
                &[key(1)]
            ),
            Err(Error::Vector),
        );
        assert!(
            verify_slot(&valid, 0x1000, Component::Lite1SecureApp, &[key(2)]).is_err(),
            "retired release key cannot verify slot"
        );
    }

    fn record(generation: u16, roots: RootSet, signers: &[u8]) -> std::vec::Vec<u8> {
        let list = TrustList::new(generation, &[key(90)], roots).unwrap();
        let message = list.signed_message();
        let mut record = std::vec::Vec::from(list.to_payload());
        record.push(signers.len() as u8);
        record.extend_from_slice(&[0; 3]);
        for signer in signers {
            record.extend_from_slice(&key(*signer));
            record.extend_from_slice(&Keypair::from(&[*signer; 32]).sign(&message).to_bytes());
        }
        record
    }

    fn page(inner: &[u8]) -> std::vec::Vec<u8> {
        let header = signed_header(Component::Lite1TrustList, inner, 90);
        let mut page = std::vec::Vec::from(header);
        page.extend_from_slice(inner);
        page.resize(2048, 0xff);
        page
    }

    fn ok(_: &TrustList) -> Result<(), Error> {
        Ok(())
    }

    fn roots(threshold: u8, seeds: &[u8]) -> RootSet {
        let mut keys = [[0; 32]; 4];
        for (slot, seed) in keys.iter_mut().zip(seeds) {
            *slot = key(*seed);
        }
        RootSet::new(threshold, keys).unwrap()
    }

    fn state_page(sequence: u32, policy: &TrustList) -> std::vec::Vec<u8> {
        let mut page = std::vec![0xff; 4096];
        page[..STATE_LEN].copy_from_slice(&state::encode(sequence, policy));
        page
    }

    fn blank() -> [std::vec::Vec<u8>; 2] {
        [std::vec![0xff; 4096], std::vec![0xff; 4096]]
    }

    fn plan(
        pages: &[std::vec::Vec<u8>; 2],
        staged: &[u8],
        anchors: &RootSet,
    ) -> Result<(TrustList, Option<state::Commit>), Error> {
        state::plan([&pages[0], &pages[1]], staged, anchors, ok)
    }

    /// Boots once and programs any commit, as stage1 does.
    fn boot(
        pages: &mut [std::vec::Vec<u8>; 2],
        staged: &[u8],
        anchors: &RootSet,
    ) -> Result<TrustList, Error> {
        let (policy, commit) = plan(pages, staged, anchors)?;
        if let Some(commit) = commit {
            pages[commit.page] = state_page(commit.sequence, &commit.policy);
        }
        Ok(policy)
    }

    #[test]
    fn next_policy_needs_exact_generation_and_current_threshold() {
        let roots = roots(2, &[1, 2]);
        let next = page(&record(2, roots, &[1, 2]));
        assert_eq!(next_policy(&next, 1, &roots).unwrap().generation(), 2);
        assert_eq!(next_policy(&next, 2, &roots), Err(Error::PolicyGeneration));
        assert!(next_policy(&page(&record(2, roots, &[1])), 1, &roots).is_err());
        assert_eq!(
            next_policy(&page(&record(1, roots, &[1, 2])), 1, &roots),
            Err(Error::PolicyGeneration)
        );
    }

    #[test]
    fn power_up_self_test_passes() {
        assert_eq!(key(0x4b), KAT_KEY);
        assert_eq!(self_test(), Ok(()));
    }

    #[test]
    fn fresh_unit_takes_only_the_factory_record_signed_by_its_anchors() {
        let anchors = roots(2, &[1, 2, 3]);
        let factory = page(&record(1, anchors, &[1, 2]));
        let (policy, commit) = plan(&blank(), &factory, &anchors).unwrap();
        assert_eq!(policy.generation(), 1);
        assert_eq!(
            commit.map(|commit| (commit.page, commit.sequence)),
            Some((0, 1))
        );
        assert!(plan(&blank(), &page(&record(1, anchors, &[1])), &anchors).is_err());
        assert!(plan(&blank(), &page(&record(2, anchors, &[1, 2])), &anchors).is_err());
        let other_roots = roots(2, &[1, 2, 4]);
        assert!(
            plan(
                &blank(),
                &page(&record(1, other_roots, &[1, 2, 4])),
                &anchors
            )
            .is_err()
        );
    }

    #[test]
    fn only_the_next_generation_under_the_stored_roots_replaces_the_policy() {
        let current_roots = roots(2, &[1, 2]);
        let current = TrustList::new(1, &[key(90)], current_roots).unwrap();
        let pages = [state_page(5, &current), std::vec![0xff; 4096]];
        let next = page(&record(2, current_roots, &[1, 2]));
        let (policy, commit) = plan(&pages, &next, &current_roots).unwrap();
        assert_eq!(policy.generation(), 2);
        assert_eq!(
            commit.map(|commit| (commit.page, commit.sequence)),
            Some((1, 6))
        );
        for ignored in [
            page(&record(1, current_roots, &[1, 2])),
            page(&record(3, current_roots, &[1, 2])),
            page(&record(2, current_roots, &[1])),
        ] {
            let (policy, commit) = plan(&pages, &ignored, &current_roots).unwrap();
            assert_eq!((policy, commit), (current, None));
        }
        let refused = state::plan([&pages[0], &pages[1]], &next, &current_roots, |_| {
            Err(Error::Slot)
        });
        assert_eq!(refused.unwrap(), (current, None));
    }

    #[test]
    fn torn_state_pages_fall_back_to_the_other_page() {
        let current_roots = roots(1, &[1]);
        let old = TrustList::new(1, &[key(90)], current_roots).unwrap();
        let new = TrustList::new(2, &[key(90)], current_roots).unwrap();
        let mut torn = state_page(2, &new);
        torn[STATE_LEN - 1] ^= 1;
        let pages = [state_page(1, &old), torn];
        assert_eq!(
            state::select([&pages[0], &pages[1]])
                .unwrap()
                .unwrap()
                .policy,
            old
        );
        let mut halfway = state_page(2, &new);
        halfway[STATE_LEN / 2..].fill(0xff);
        let pages = [halfway, state_page(1, &old)];
        assert_eq!(
            state::select([&pages[0], &pages[1]])
                .unwrap()
                .unwrap()
                .policy,
            old
        );
        let both = [state_page(3, &old), state_page(3, &new)];
        assert_eq!(
            state::select([&both[0], &both[1]]),
            Err(Error::AmbiguousPolicy)
        );
        let exhausted = [state_page(u32::MAX - 1, &old), std::vec![0xff; 4096]];
        let next = page(&record(2, current_roots, &[1]));
        assert_eq!(
            plan(&exhausted, &next, &current_roots),
            Err(Error::Generation)
        );
    }

    #[test]
    fn revoked_root_cannot_authorize_a_replacement() {
        let anchors = roots(2, &[1, 2, 3]);
        let mut pages = blank();
        boot(&mut pages, &page(&record(1, anchors, &[1, 2])), &anchors).unwrap();
        let rotated = roots(2, &[2, 3, 4]);
        let policy = boot(&mut pages, &page(&record(2, rotated, &[2, 3, 4])), &anchors).unwrap();
        assert_eq!(policy.root_set(), &rotated);
        let attacker = roots(2, &[1, 9]);
        for forged in [
            record(3, attacker, &[1, 9]),
            record(3, roots(2, &[1, 2, 9]), &[1, 2, 9]),
            record(3, rotated, &[1, 9]),
            record(3, rotated, &[1, 2]),
        ] {
            let kept = boot(&mut pages, &page(&forged), &anchors).unwrap();
            assert_eq!(kept.generation(), 2, "forged record must not apply");
        }
        let again = boot(&mut pages, &page(&record(3, rotated, &[3, 4])), &anchors).unwrap();
        assert_eq!(again.generation(), 3);
    }

    #[test]
    fn many_root_rotations_are_not_limited() {
        let anchors = roots(1, &[1]);
        let mut pages = blank();
        boot(&mut pages, &page(&record(1, anchors, &[1])), &anchors).unwrap();
        for index in 1..200u8 {
            let next = roots(1, &[index + 1]);
            let staged = page(&record(u16::from(index) + 1, next, &[index, index + 1]));
            let policy = boot(&mut pages, &staged, &anchors).unwrap();
            assert_eq!(policy.root_set(), &next);
        }
        let stored = state::select([&pages[0], &pages[1]]).unwrap().unwrap();
        assert_eq!((stored.policy.generation(), stored.sequence), (200, 200));
        assert_eq!(stored.page, 1, "commits alternate pages");
    }
}
