/// Migration identity for shipped bootloader counters. Unknown counters stay unknown.
///
/// `0.0.1` through `0.0.5` identify counter releases, not their Cargo versions.
///
/// Since: Odyssey Desktop (unreleased).
pub const fn legacy_bootloader_version(counter: u16) -> Option<[u32; 3]> {
    match counter {
        1..=5 => Some([0, 0, counter as u32]),
        _ => None,
    }
}

/// Historical counter tag for a reserved migration identity, never a new release.
///
/// Since: Odyssey Desktop (unreleased).
pub const fn legacy_bootloader_counter(version: [u32; 3]) -> Option<u16> {
    match version {
        [0, 0, patch @ 1..=5] => Some(patch as u16),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{legacy_bootloader_counter, legacy_bootloader_version};

    #[test]
    fn shipped_counter_identities_preserve_release_order_and_tags() {
        let mut previous = [0; 3];
        for counter in 1..=5 {
            let version =
                legacy_bootloader_version(counter).expect("shipped counter has an identity");
            assert!(version > previous);
            assert_eq!(legacy_bootloader_counter(version), Some(counter));
            previous = version;
        }
    }

    #[test]
    fn unknown_counters_and_nonmigration_versions_have_no_alias() {
        for counter in [0, 6, u16::MAX] {
            assert_eq!(legacy_bootloader_version(counter), None);
        }
        for version in [[0, 0, 0], [0, 0, 6], [0, 1, 1], [1, 0, 1], [0, 0, u32::MAX]] {
            assert_eq!(legacy_bootloader_counter(version), None);
        }
    }
}
