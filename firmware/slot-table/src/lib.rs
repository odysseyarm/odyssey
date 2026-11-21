//! Where an entry goes in a small fixed table of keyed entries, such as a list of bonds.
//!
//! The caller says how each slot relates to the entry it wants to store ([`Slot`]) and
//! [`slot_for`] picks the slot. Verus proves the choice for every table size:
//!
//! - an entry already in the table is stored over itself, so it is never stored twice;
//! - otherwise it takes the first free slot;
//! - a slot holding another entry is never chosen, so storing never evicts one;
//! - the only refusal is a table full of other entries.
//!
//! Run the proof with `tools/verify-verus.sh` and the pinned verifier on `PATH`. The gate
//! rejects unproved assumptions and skipped executable bodies.

#![no_std]
#![cfg_attr(verus_only, feature(proc_macro_hygiene))]

#[cfg(verus_only)]
use vstd::prelude::*;

/// What a slot holds, relative to the entry about to be stored.
#[cfg_attr(verus_only, verus_verify, derive(Structural))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    /// Nothing.
    Free,
    /// The entry about to be stored.
    Same,
    /// Another entry.
    Other,
}

/// The slot the entry is stored in, or `None` when every slot holds another entry.
#[cfg_attr(verus_only, verifier::loop_isolation(false))]
#[cfg_attr(verus_only, verus_spec(result =>
    ensures
        match result {
            Some(chosen) => chosen < N
                && !(slots@[chosen as int] is Other)
                && (slots@[chosen as int] is Free ==>
                    forall|j: int| 0 <= j < N ==> !(#[trigger] slots@[j] is Same))
                && forall|j: int| 0 <= j < chosen ==> #[trigger] slots@[j] != slots@[chosen as int],
            None => forall|j: int| 0 <= j < N ==> #[trigger] slots@[j] is Other,
        },
))]
#[must_use]
#[expect(
    clippy::needless_range_loop,
    reason = "the Verus loop invariants are stated over the index"
)]
pub fn slot_for<const N: usize>(slots: &[Slot; N]) -> Option<usize> {
    let mut free: Option<usize> = None;
    #[cfg_attr(verus_only, verus_spec(
        invariant
            forall|j: int| 0 <= j < i ==> !(#[trigger] slots@[j] is Same),
            match free {
                Some(first) => first < i
                    && slots@[first as int] is Free
                    && forall|j: int| 0 <= j < first ==> !(#[trigger] slots@[j] is Free),
                None => forall|j: int| 0 <= j < i ==> !(#[trigger] slots@[j] is Free),
            },
    ))]
    for i in 0..N {
        match slots[i] {
            Slot::Same => return Some(i),
            Slot::Free => {
                if free.is_none() {
                    free = Some(i);
                }
            }
            Slot::Other => {}
        }
    }
    free
}

#[cfg(test)]
mod tests {
    use super::{Slot, slot_for};

    #[test]
    fn an_entry_already_stored_wins_over_an_earlier_free_slot() {
        assert_eq!(slot_for(&[Slot::Free, Slot::Other, Slot::Same]), Some(2));
        assert_eq!(slot_for(&[Slot::Other, Slot::Free, Slot::Free]), Some(1));
        assert_eq!(slot_for(&[Slot::Other, Slot::Other]), None);
        assert_eq!(slot_for::<0>(&[]), None);
    }
}
