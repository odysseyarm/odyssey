//! Release decisions as pure functions over the component table, kept apart
//! from the tool around them so Kani can check them without external inputs:
//! `kani crates/xtask/src/release_rules.rs`, or
//! `nix build .#checks.x86_64-linux.release-proofs`.
//!
//! The file names nothing outside `std`, and nothing newer than the 2015
//! edition that a bare `kani <file>` compiles with, so it verifies on its own.

use std::cmp::Ordering;

/// How many components the table in `release.rs` lists; the proofs run at this size.
pub(crate) const COMPONENT_COUNT: usize = 11;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub(crate) enum State {
    /// Released as it is.
    Current,
    /// Changed since its tag under the tag's version.
    NeedsBump,
    /// The source is ahead of the newest tag, or was never released.
    Ready,
}

/// The source version is below one already released.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Regressed;

/// `ahead` orders the source version against the newest tag's, and is `None`
/// for a component that has no tag.
pub(crate) fn state(ahead: Option<Ordering>, changed: bool) -> Result<State, Regressed> {
    match ahead {
        None | Some(Ordering::Greater) => Ok(State::Ready),
        Some(Ordering::Less) => Err(Regressed),
        Some(Ordering::Equal) if changed => Ok(State::NeedsBump),
        Some(Ordering::Equal) => Ok(State::Current),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub(crate) enum Kind {
    /// A net core or bootloader. Units receive it through the manifest entries
    /// of the application at this table index.
    Part {
        app: usize,
    },
    App,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub(crate) struct Link {
    pub(crate) kind: Kind,
    /// Bumped only by name.
    pub(crate) manual: bool,
}

/// Every part names an application, and no application is bumped by name only:
/// it could then stay behind a part it has to deliver. Every other function
/// here requires a valid table.
pub(crate) fn valid<const N: usize>(table: &[Link; N]) -> bool {
    for i in 0..N {
        match table[i].kind {
            Kind::Part { app } => {
                if app >= N || table[app].kind != Kind::App {
                    return false;
                }
            }
            Kind::App => {
                if table[i].manual {
                    return false;
                }
            }
            Kind::Other => {}
        }
    }
    true
}

/// A unit only receives a part through an application entry that carries it, so
/// a part on its way out makes its application release too. `behind[i]` says
/// part `i`'s counter is above the one its application's newest entry carries.
/// Returns, for each application made to release, the part that forces it.
pub(crate) fn deliver<const N: usize>(
    table: &[Link; N],
    states: &mut [State; N],
    behind: &[bool; N],
) -> [Option<usize>; N] {
    let mut forced = [None; N];
    for i in 0..N {
        let Kind::Part { app } = table[i].kind else {
            continue;
        };
        let on_its_way = match states[i] {
            State::Ready => true,
            State::NeedsBump => !table[i].manual,
            State::Current => false,
        };
        if (on_its_way || behind[i]) && states[app] == State::Current {
            states[app] = State::NeedsBump;
            forced[app] = Some(i);
        }
    }
    forced
}

/// The components `bump` raises: the `named` ones, or without names every
/// changed one that is not bumped by name only; plus the application of each
/// part among them. A component already ahead of its tag is never raised again.
pub(crate) fn bump_set<const N: usize>(
    table: &[Link; N],
    states: &[State; N],
    named: Option<&[bool; N]>,
) -> [bool; N] {
    let mut wanted = [false; N];
    for i in 0..N {
        wanted[i] = match named {
            Some(named) => named[i],
            None => states[i] == State::NeedsBump && !table[i].manual,
        };
    }
    for i in 0..N {
        match table[i].kind {
            Kind::Part { app } if wanted[i] => wanted[app] = true,
            _ => {}
        }
    }
    for i in 0..N {
        if states[i] == State::Ready {
            wanted[i] = false;
        }
    }
    wanted
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Refusal {
    /// The named component is not ahead of its tag.
    NotReady(usize),
    /// No release of `app` is on its way to deliver `part`, so no unit would
    /// ever be offered it.
    NoCarrier { part: usize, app: usize },
}

/// Table indices in the order `publish` releases them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Order<const N: usize> {
    items: [usize; N],
    len: usize,
}

impl<const N: usize> Order<N> {
    pub(crate) fn as_slice(&self) -> &[usize] {
        &self.items[..self.len]
    }
}

fn rank(kind: Kind) -> u8 {
    match kind {
        Kind::Part { .. } => 0,
        Kind::App => 1,
        Kind::Other => 2,
    }
}

/// The components `publish` releases: the `named` ones, or without names every
/// one ahead of its tag. Parts come first, because an application's manifest
/// entry hashes the published net core and bootloader.
pub(crate) fn publish_order<const N: usize>(
    table: &[Link; N],
    states: &[State; N],
    named: Option<&[bool; N]>,
) -> Result<Order<N>, Refusal> {
    let mut chosen = [false; N];
    for i in 0..N {
        chosen[i] = match named {
            Some(named) => named[i],
            None => states[i] == State::Ready,
        };
        if chosen[i] && states[i] != State::Ready {
            return Err(Refusal::NotReady(i));
        }
    }
    for i in 0..N {
        match table[i].kind {
            Kind::Part { app } if chosen[i] && states[app] != State::Ready => {
                return Err(Refusal::NoCarrier { part: i, app });
            }
            _ => {}
        }
    }
    let mut order = Order {
        items: [0; N],
        len: 0,
    };
    for wave in 0..3 {
        for i in 0..N {
            if chosen[i] && rank(table[i].kind) == wave {
                order.items[order.len] = i;
                order.len += 1;
            }
        }
    }
    Ok(order)
}

/// The next counter, only when it stays within the component's representation.
pub(crate) fn successor(value: u64, maximum: u64) -> Option<u64> {
    value.checked_add(1).filter(|next| *next <= maximum)
}

/// Index of the oldest Desktop release from which every later one speaks at
/// least `want`. `speaks` is oldest first; `None` marks a release without the
/// wire constant.
pub(crate) fn oldest_covering(want: [u64; 3], speaks: &[Option<[u64; 3]>]) -> Option<usize> {
    let mut floor = None;
    let mut i = speaks.len();
    while i > 0 {
        i -= 1;
        match speaks[i] {
            Some(version) if version >= want => floor = Some(i),
            _ => break,
        }
    }
    floor
}

#[cfg(kani)]
mod proofs {
    use super::*;

    const N: usize = COMPONENT_COUNT;
    /// `#[kani::unwind]` takes a literal: one more than the longest loop.
    const _: () = {
        assert!(N + 1 == 12);
    };
    const DESKTOPS: usize = 5;

    fn any_table() -> [Link; N] {
        let table: [Link; N] = kani::any();
        kani::assume(valid(&table));
        table
    }

    /// What `plan` hands to `bump` and `publish`: any states, after delivery.
    fn any_plan(table: &[Link; N]) -> ([State; N], [bool; N]) {
        let mut states: [State; N] = kani::any();
        let behind: [bool; N] = kani::any();
        deliver(table, &mut states, &behind);
        (states, behind)
    }

    fn on_its_way(table: &[Link; N], states: &[State; N], i: usize) -> bool {
        states[i] == State::Ready || (states[i] == State::NeedsBump && !table[i].manual)
    }

    #[kani::proof]
    fn a_changed_component_is_never_current_and_a_regression_is_refused() {
        let a: u8 = kani::any();
        let b: u8 = kani::any();
        let ahead = if kani::any() { Some(a.cmp(&b)) } else { None };
        let changed: bool = kani::any();
        let state = state(ahead, changed);
        assert_eq!(state.is_err(), ahead == Some(Ordering::Less));
        if changed {
            assert!(state != Ok(State::Current));
        }
        if ahead == Some(Ordering::Equal) && !changed {
            assert!(state == Ok(State::Current));
        }
        if ahead.is_none() || ahead == Some(Ordering::Greater) {
            assert!(state == Ok(State::Ready));
        }
    }

    #[kani::proof]
    #[kani::unwind(12)]
    fn delivery_leaves_no_outgoing_part_without_a_releasing_application() {
        let table = any_table();
        let before: [State; N] = kani::any();
        let behind: [bool; N] = kani::any();
        let mut states = before;
        let forced = deliver(&table, &mut states, &behind);
        for i in 0..N {
            if let Kind::Part { app } = table[i].kind {
                if on_its_way(&table, &states, i) || behind[i] {
                    assert!(states[app] != State::Current);
                }
            }
            let raised = before[i] == State::Current && states[i] == State::NeedsBump;
            assert!(states[i] == before[i] || (raised && table[i].kind == Kind::App));
            assert_eq!(forced[i].is_some(), raised);
        }
        let mut again = states;
        deliver(&table, &mut again, &behind);
        assert!(again == states);
    }

    #[kani::proof]
    #[kani::unwind(12)]
    fn one_bump_leaves_nothing_to_bump_and_publish_then_accepts_everything_ready() {
        let table = any_table();
        let (states, behind) = any_plan(&table);
        let set = bump_set(&table, &states, None);
        let mut after = states;
        let mut behind_after = behind;
        for i in 0..N {
            if states[i] == State::NeedsBump && !table[i].manual {
                assert!(set[i]);
            }
            if set[i] {
                assert!(states[i] != State::Ready);
                after[i] = State::Ready;
                if let Kind::Part { app } = table[i].kind {
                    assert!(set[app] || states[app] == State::Ready);
                    behind_after[i] = true;
                }
            }
        }
        deliver(&table, &mut after, &behind_after);
        let again = bump_set(&table, &after, None);
        for i in 0..N {
            assert!(!again[i]);
        }
        let Ok(order) = publish_order(&table, &after, None) else {
            panic!("publish refused a plan that bump had settled");
        };
        let mut ready = 0;
        for i in 0..N {
            if after[i] == State::Ready {
                ready += 1;
            }
        }
        assert_eq!(order.len, ready);
    }

    #[kani::proof]
    #[kani::unwind(12)]
    fn a_named_bump_brings_the_application_of_each_part() {
        let table = any_table();
        let (states, _) = any_plan(&table);
        let named: [bool; N] = kani::any();
        let set = bump_set(&table, &states, Some(&named));
        for i in 0..N {
            if named[i] && states[i] != State::Ready {
                assert!(set[i]);
            }
            if set[i] {
                assert!(states[i] != State::Ready);
                if let Kind::Part { app } = table[i].kind {
                    assert!(set[app] || states[app] == State::Ready);
                }
            }
        }
    }

    #[kani::proof]
    #[kani::unwind(12)]
    fn publish_releases_only_what_is_ready_with_its_carrier_and_parts_first() {
        let table = any_table();
        let states: [State; N] = kani::any();
        let named: Option<[bool; N]> = kani::any();
        let Ok(order) = publish_order(&table, &states, named.as_ref()) else {
            return;
        };
        assert!(order.len <= N);
        let mut seen = [false; N];
        let mut wave = 0;
        for position in 0..order.len {
            let i = order.items[position];
            assert!(!seen[i]);
            seen[i] = true;
            assert!(states[i] == State::Ready);
            if let Kind::Part { app } = table[i].kind {
                assert!(states[app] == State::Ready);
            }
            assert!(rank(table[i].kind) >= wave);
            wave = rank(table[i].kind);
        }
        for i in 0..N {
            let wanted = match &named {
                Some(named) => named[i],
                None => states[i] == State::Ready,
            };
            assert_eq!(seen[i], wanted);
            if named.is_none() && seen[i] {
                if let Kind::Part { app } = table[i].kind {
                    assert!(seen[app]);
                }
            }
        }
    }

    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::solver(kissat)]
    fn the_desktop_floor_covers_every_later_release_and_is_the_oldest_that_does() {
        let want: [u64; 3] = kani::any();
        let all: [Option<[u64; 3]>; DESKTOPS] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= DESKTOPS);
        let speaks = &all[..len];
        let covers = |i: usize| speaks[i].is_some_and(|version| version >= want);
        match oldest_covering(want, speaks) {
            Some(floor) => {
                assert!(floor < len);
                let later: usize = kani::any();
                kani::assume(later >= floor && later < len);
                assert!(covers(later));
                assert!(floor == 0 || !covers(floor - 1));
            }
            None => {
                assert!(len == 0 || !covers(len - 1));
            }
        }
    }

    #[kani::proof]
    fn successor_matches_checked_ordered_increment_for_all_u64_pairs() {
        let value: u64 = kani::any();
        let maximum: u64 = kani::any();
        let expected = if value < maximum {
            Some(value + 1)
        } else {
            None
        };
        assert_eq!(successor(value, maximum), expected);
    }
}
