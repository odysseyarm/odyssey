//! Signed trust policy: everyday release keys plus the recovery root set that
//! authorizes the next policy.
//!
//! A policy record is the payload followed by up to [`MAX_SIGNATURES`] entries of
//! signer public key and Ed25519 signature, all over [`TrustList::signed_message`].
//! A record is accepted only when at least `threshold` distinct roots of the
//! current root set signed it. A record that changes the root set must also be
//! signed by `threshold` of its own new roots, so every root change proves the
//! new keys work.

use crate::hash::Sha256;
use crate::keys::{KEY_SLOTS, PublicKey};
use crate::message::{Component, MESSAGE_LEN, message};
use crate::verify::{SIGNATURE_LEN, verify_with_keys};

/// Trust-list payload length.
pub const TRUST_LIST_LEN: usize = 272;
/// Most signatures a policy record carries.
pub const MAX_SIGNATURES: usize = 8;
/// Bytes per signature entry: signer public key, then signature.
pub const SIGNATURE_ENTRY_LEN: usize = 32 + SIGNATURE_LEN;
/// Bytes before the first signature entry.
pub const RECORD_HEADER_LEN: usize = TRUST_LIST_LEN + 4;
/// Largest policy record.
pub const POLICY_RECORD_MAX: usize = RECORD_HEADER_LEN + MAX_SIGNATURES * SIGNATURE_ENTRY_LEN;
const MAGIC: &[u8; 8] = b"ODYTRST2";
const EMPTY: PublicKey = [0; 32];

/// Invalid trust-list format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustListError {
    Header,
    Length,
    Generation,
    KeyCount,
    Threshold,
    Reserved,
    EmptyKey,
    DuplicateKey,
    Signatures,
}

impl core::fmt::Display for TrustListError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl core::error::Error for TrustListError {}

/// Recovery roots and how many of them must sign a policy change. Empty slots
/// are all-zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootSet {
    threshold: u8,
    roots: [PublicKey; KEY_SLOTS],
}

impl RootSet {
    /// Validates distinct non-empty roots and `1 <= threshold <= root count`.
    pub fn new(threshold: u8, roots: [PublicKey; KEY_SLOTS]) -> Result<Self, TrustListError> {
        let mut count = 0;
        for (i, root) in roots.iter().enumerate() {
            if *root != EMPTY {
                if roots[..i].contains(root) {
                    return Err(TrustListError::DuplicateKey);
                }
                count += 1;
            }
        }
        if threshold == 0 || threshold > count {
            return Err(TrustListError::Threshold);
        }
        Ok(Self { threshold, roots })
    }

    #[must_use]
    pub const fn threshold(&self) -> u8 {
        self.threshold
    }

    #[must_use]
    pub const fn roots(&self) -> &[PublicKey; KEY_SLOTS] {
        &self.roots
    }

    /// Whether at least `threshold` distinct roots have a valid entry over `message`.
    fn signed(&self, message: &[u8; MESSAGE_LEN], entries: &[u8]) -> bool {
        let mut signed = 0;
        for root in self.roots.iter().filter(|root| **root != EMPTY) {
            let valid = entries.chunks_exact(SIGNATURE_ENTRY_LEN).any(|entry| {
                entry[..32] == *root
                    && entry[32..]
                        .try_into()
                        .is_ok_and(|signature| verify_with_keys(&[*root], message, signature))
            });
            if valid {
                signed += 1;
                if signed >= self.threshold {
                    return true;
                }
            }
        }
        false
    }
}

/// Everyday release keys and the root set for the next policy change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrustList {
    generation: u16,
    key_count: u8,
    keys: [PublicKey; KEY_SLOTS],
    roots: RootSet,
}

impl TrustList {
    /// Validates generation and distinct non-empty release keys.
    pub fn new(
        generation: u16,
        keys: &[PublicKey],
        roots: RootSet,
    ) -> Result<Self, TrustListError> {
        if !(1..=65534).contains(&generation) {
            return Err(TrustListError::Generation);
        }
        if keys.is_empty() || keys.len() > KEY_SLOTS {
            return Err(TrustListError::KeyCount);
        }
        let mut slots = [EMPTY; KEY_SLOTS];
        for (i, key) in keys.iter().enumerate() {
            if *key == EMPTY {
                return Err(TrustListError::EmptyKey);
            }
            if keys[..i].contains(key) {
                return Err(TrustListError::DuplicateKey);
            }
            slots[i] = *key;
        }
        Ok(Self {
            generation,
            key_count: keys.len() as u8,
            keys: slots,
            roots,
        })
    }

    /// Parses and validates exact payload bytes.
    pub fn parse(payload: &[u8]) -> Result<Self, TrustListError> {
        let payload: &[u8; TRUST_LIST_LEN] =
            payload.try_into().map_err(|_| TrustListError::Length)?;
        if &payload[..8] != MAGIC {
            return Err(TrustListError::Header);
        }
        if payload[12..16].iter().any(|byte| *byte != 0) {
            return Err(TrustListError::Reserved);
        }
        let key_count = usize::from(payload[10]);
        if !(1..=KEY_SLOTS).contains(&key_count) {
            return Err(TrustListError::KeyCount);
        }
        let mut keys = [EMPTY; KEY_SLOTS];
        let mut roots = [EMPTY; KEY_SLOTS];
        for i in 0..KEY_SLOTS {
            keys[i].copy_from_slice(&payload[16 + 32 * i..48 + 32 * i]);
            roots[i].copy_from_slice(&payload[144 + 32 * i..176 + 32 * i]);
            if i >= key_count && keys[i] != EMPTY {
                return Err(TrustListError::Reserved);
            }
        }
        let list = Self::new(
            u16::from_le_bytes([payload[8], payload[9]]),
            &keys[..key_count],
            RootSet::new(payload[11], roots)?,
        )?;
        Ok(list)
    }

    /// Encodes exact payload bytes.
    #[must_use]
    pub fn to_payload(&self) -> [u8; TRUST_LIST_LEN] {
        let mut out = [0; TRUST_LIST_LEN];
        out[..8].copy_from_slice(MAGIC);
        out[8..10].copy_from_slice(&self.generation.to_le_bytes());
        out[10] = self.key_count;
        out[11] = self.roots.threshold;
        for i in 0..KEY_SLOTS {
            out[16 + 32 * i..48 + 32 * i].copy_from_slice(&self.keys[i]);
            out[144 + 32 * i..176 + 32 * i].copy_from_slice(&self.roots.roots[i]);
        }
        out
    }

    /// Message every signature entry signs: binds payload hash and generation.
    #[must_use]
    pub fn signed_message(&self) -> [u8; MESSAGE_LEN] {
        let mut hash = Sha256::new();
        hash.update(&self.to_payload());
        message(
            Component::Lite1TrustList,
            TRUST_LIST_LEN as u32,
            [u32::from(self.generation), 0, 0],
            &hash.finalize(),
        )
    }

    #[must_use]
    pub const fn generation(&self) -> u16 {
        self.generation
    }

    /// Active release keys only.
    #[must_use]
    pub fn keys(&self) -> &[PublicKey] {
        &self.keys[..self.key_count as usize]
    }

    #[must_use]
    pub const fn root_set(&self) -> &RootSet {
        &self.roots
    }

    /// Parses a signed record and returns it with its signature entries. Bytes
    /// after the entries, up to the end of `record`, must be erased (0xFF).
    pub fn parse_record(record: &[u8]) -> Result<(Self, &[u8]), TrustListError> {
        if record.len() < RECORD_HEADER_LEN {
            return Err(TrustListError::Length);
        }
        let list = Self::parse(&record[..TRUST_LIST_LEN])?;
        let count = usize::from(record[TRUST_LIST_LEN]);
        if !(1..=MAX_SIGNATURES).contains(&count)
            || record[TRUST_LIST_LEN + 1..RECORD_HEADER_LEN] != [0; 3]
        {
            return Err(TrustListError::Signatures);
        }
        let end = RECORD_HEADER_LEN + count * SIGNATURE_ENTRY_LEN;
        let entries = record
            .get(RECORD_HEADER_LEN..end)
            .ok_or(TrustListError::Length)?;
        if record[end..].iter().any(|byte| *byte != 0xFF) {
            return Err(TrustListError::Reserved);
        }
        Ok((list, entries))
    }

    /// Whether `entries` authorize this list under the `current` root set: a
    /// threshold of current roots, plus a threshold of new roots when the set changes.
    #[must_use]
    pub fn authorized_by(&self, current: &RootSet, entries: &[u8]) -> bool {
        let message = self.signed_message();
        current.signed(&message, entries)
            && (self.roots == *current || self.roots.signed(&message, entries))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;

    use salty::Keypair;

    use super::*;

    fn key(seed: u8) -> PublicKey {
        Keypair::from(&[seed; 32]).public.to_bytes()
    }

    fn roots(threshold: u8, seeds: &[u8]) -> RootSet {
        let mut set = [EMPTY; KEY_SLOTS];
        for (slot, seed) in set.iter_mut().zip(seeds) {
            *slot = key(*seed);
        }
        RootSet::new(threshold, set).unwrap()
    }

    fn record(list: &TrustList, signers: &[u8]) -> Vec<u8> {
        let message = list.signed_message();
        let mut out = list.to_payload().to_vec();
        out.extend([signers.len() as u8, 0, 0, 0]);
        for seed in signers {
            out.extend(key(*seed));
            out.extend(Keypair::from(&[*seed; 32]).sign(&message).to_bytes());
        }
        out
    }

    #[test]
    fn threshold_counts_distinct_valid_roots() {
        let current = roots(2, &[1, 2, 3]);
        let list = TrustList::new(2, &[key(9)], current).unwrap();
        let signed = record(&list, &[1, 3]);
        let (parsed, entries) = TrustList::parse_record(&signed).unwrap();
        assert_eq!(parsed, list);
        assert!(parsed.authorized_by(&current, entries));
        let one = record(&list, &[1]);
        let (_, entries) = TrustList::parse_record(&one).unwrap();
        assert!(
            !list.authorized_by(&current, entries),
            "one root is below threshold"
        );
        let twice = record(&list, &[1, 1]);
        let (_, entries) = TrustList::parse_record(&twice).unwrap();
        assert!(!list.authorized_by(&current, entries), "a root counts once");
        let outsider = record(&list, &[1, 7]);
        let (_, entries) = TrustList::parse_record(&outsider).unwrap();
        assert!(
            !list.authorized_by(&current, entries),
            "non-roots do not count"
        );
    }

    #[test]
    fn root_change_needs_old_and_new_thresholds() {
        let current = roots(2, &[1, 2, 3]);
        let next = TrustList::new(2, &[key(9)], roots(2, &[2, 3, 4])).unwrap();
        let signed = record(&next, &[1, 2]);
        let (_, entries) = TrustList::parse_record(&signed).unwrap();
        assert!(
            !next.authorized_by(&current, entries),
            "new roots must sign too"
        );
        let signed = record(&next, &[1, 2, 4]);
        let (_, entries) = TrustList::parse_record(&signed).unwrap();
        assert!(next.authorized_by(&current, entries));
        let signed = record(&next, &[2, 4]);
        let (_, entries) = TrustList::parse_record(&signed).unwrap();
        assert!(
            !next.authorized_by(&current, entries),
            "only one old root signed"
        );
    }

    #[test]
    fn disjoint_full_root_sets_can_rotate() {
        let old = roots(4, &[1, 2, 3, 4]);
        let new = roots(4, &[5, 6, 7, 8]);
        let next = TrustList::new(2, &[key(9)], new).unwrap();
        let signed = record(&next, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(signed.len(), POLICY_RECORD_MAX);
        let (_, entries) = TrustList::parse_record(&signed).unwrap();
        assert!(next.authorized_by(&old, entries));
    }

    #[test]
    fn tampered_payload_or_signature_fails() {
        let current = roots(1, &[1]);
        let list = TrustList::new(3, &[key(9)], current).unwrap();
        let good = record(&list, &[1]);
        let mut payload = good.clone();
        payload[16] ^= 1;
        let (changed, entries) = TrustList::parse_record(&payload).unwrap();
        assert!(!changed.authorized_by(&current, entries));
        let mut signature = good.clone();
        *signature.last_mut().unwrap() ^= 1;
        let (list, entries) = TrustList::parse_record(&signature).unwrap();
        assert!(!list.authorized_by(&current, entries));
    }

    #[test]
    fn parser_rejects_malformed_records() {
        let list = TrustList::new(3, &[key(9)], roots(1, &[1])).unwrap();
        let good = record(&list, &[1]);
        let mut trailing = good.clone();
        trailing.extend([0xFF; 8]);
        assert!(
            TrustList::parse_record(&trailing).is_ok(),
            "erased tail is accepted"
        );
        trailing.push(0);
        assert_eq!(
            TrustList::parse_record(&trailing),
            Err(TrustListError::Reserved)
        );
        let mut none = good.clone();
        none[TRUST_LIST_LEN] = 0;
        assert_eq!(
            TrustList::parse_record(&none),
            Err(TrustListError::Signatures)
        );
        assert_eq!(
            TrustList::parse_record(&good[..300]),
            Err(TrustListError::Length)
        );
        let mut threshold = good.clone();
        threshold[11] = 2;
        assert_eq!(
            TrustList::parse_record(&threshold),
            Err(TrustListError::Threshold)
        );
        let mut duplicate = good;
        duplicate[176..208].copy_from_slice(&key(1));
        assert_eq!(
            TrustList::parse_record(&duplicate),
            Err(TrustListError::DuplicateKey)
        );
        assert_eq!(
            RootSet::new(1, [EMPTY; KEY_SLOTS]),
            Err(TrustListError::Threshold)
        );
    }
}
