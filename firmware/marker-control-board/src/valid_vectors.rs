/// Reject erased/truncated vectors, non-Thumb entrypoints and addresses outside this image.
///
/// Since: Odyssey Desktop 0.7.4.
pub fn valid_vectors(
    sp: u32,
    reset: u32,
    active: core::ops::Range<u32>,
    ram: core::ops::Range<u32>,
) -> bool {
    let Some(first_instruction) = active.start.checked_add(8) else {
        return false;
    };
    sp > ram.start
        && sp <= ram.end
        && sp % 8 == 0
        && reset & 1 != 0
        && reset & !1 >= first_instruction
        && reset & !1 < active.end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_bounds_reject_missing_and_unbootable_app() {
        let valid = |sp, pc| valid_vectors(sp, pc, 0x11000..0x48000, 0x20000000..0x20020000);
        assert!(valid(0x20020000, 0x11009));
        for (sp, pc) in [
            (u32::MAX, u32::MAX),
            (0x20020008, 0x11009),
            (0x20000000, 0x11009),
            (0x20010004, 0x11009),
            (0x20010000, 0x11008),
            (0x20010000, 0x11001),
            (0x20010000, 0x48001),
        ] {
            assert!(!valid(sp, pc), "sp={sp:#x}, pc={pc:#x}");
        }
        assert!(!valid_vectors(
            0x20020000,
            0x11009,
            u32::MAX..u32::MAX,
            0x20000000..0x20020000
        ));
    }
}
