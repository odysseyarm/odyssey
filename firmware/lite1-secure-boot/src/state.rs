//! Accepted trust policy, kept in two flash pages that only stage1 writes and
//! write-protects before handing off. Replaces a fixed history of root rotations:
//! the page holds the latest accepted roots, so there is no lifetime limit.

use release_signing::{RootSet, Sha256, TRUST_LIST_LEN, TrustList};

use crate::{Error, next_policy, verify_policy_record};

const MAGIC: [u8; 4] = *b"OTS1";
const BODY_LEN: usize = 4 + 4 + TRUST_LIST_LEN;
/// Bytes of one state record: magic, sequence, trust-list payload, then SHA-256 of the rest.
pub const STATE_LEN: usize = BODY_LEN + 32;

/// Encodes the record stage1 programs after it accepts `policy`.
#[must_use]
pub fn encode(sequence: u32, policy: &TrustList) -> [u8; STATE_LEN] {
    let mut record = [0; STATE_LEN];
    record[..4].copy_from_slice(&MAGIC);
    record[4..8].copy_from_slice(&sequence.to_le_bytes());
    record[8..BODY_LEN].copy_from_slice(&policy.to_payload());
    let mut hash = Sha256::new();
    hash.update(&record[..BODY_LEN]);
    record[BODY_LEN..].copy_from_slice(&hash.finalize());
    record
}

/// A page decodes only when complete: a write cut by power loss fails the hash.
fn decode(page: &[u8]) -> Option<(u32, TrustList)> {
    let record = page.get(..STATE_LEN)?;
    if record[..4] != MAGIC {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(&record[..BODY_LEN]);
    if hash.finalize()[..] != record[BODY_LEN..] {
        return None;
    }
    let sequence = u32::from_le_bytes(record[4..8].try_into().ok()?);
    if sequence == 0 || sequence == u32::MAX {
        return None;
    }
    Some((sequence, TrustList::parse(&record[8..BODY_LEN]).ok()?))
}

/// Newest complete record and the page holding it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stored {
    pub page: usize,
    pub sequence: u32,
    pub policy: TrustList,
}

/// Picks the newest complete record. `None` on a unit stage1 has not initialized.
pub fn select(pages: [&[u8]; 2]) -> Result<Option<Stored>, Error> {
    let stored = |page, (sequence, policy)| Stored {
        page,
        sequence,
        policy,
    };
    match (decode(pages[0]), decode(pages[1])) {
        (None, None) => Ok(None),
        (Some(a), None) => Ok(Some(stored(0, a))),
        (None, Some(b)) => Ok(Some(stored(1, b))),
        (Some(a), Some(b)) if a.0 == b.0 => Err(Error::AmbiguousPolicy),
        (Some(a), Some(b)) => Ok(Some(if a.0 > b.0 {
            stored(0, a)
        } else {
            stored(1, b)
        })),
    }
}

/// A record stage1 must program into `page` before it uses `policy`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Commit {
    pub page: usize,
    pub sequence: u32,
    pub policy: TrustList,
}

/// Decides this boot's policy from the stored state and the staged record.
///
/// A fresh unit takes the factory record, signed by the immutable `anchors`.
/// Afterwards only the next generation, signed under the stored roots, replaces
/// the stored policy; a stale, forged or unusable staged record is ignored. The
/// commit goes to the page not holding the current record, so a power cut leaves
/// the old record in force and the next boot repeats the commit.
pub fn plan(
    pages: [&[u8]; 2],
    staged: &[u8],
    anchors: &RootSet,
    usable: impl Fn(&TrustList) -> Result<(), Error>,
) -> Result<(TrustList, Option<Commit>), Error> {
    let Some(stored) = select(pages)? else {
        let policy = verify_policy_record(staged, 1, anchors)?;
        if policy.root_set() != anchors {
            return Err(Error::PolicyGeneration);
        }
        usable(&policy)?;
        let commit = Commit {
            page: 0,
            sequence: 1,
            policy,
        };
        return Ok((policy, Some(commit)));
    };
    let current = stored.policy;
    let Ok(next) = next_policy(staged, current.generation(), current.root_set()) else {
        return Ok((current, None));
    };
    if usable(&next).is_err() {
        return Ok((current, None));
    }
    let sequence = stored
        .sequence
        .checked_add(1)
        .filter(|sequence| *sequence != u32::MAX)
        .ok_or(Error::Generation)?;
    let commit = Commit {
        page: 1 - stored.page,
        sequence,
        policy: next,
    };
    Ok((next, Some(commit)))
}
