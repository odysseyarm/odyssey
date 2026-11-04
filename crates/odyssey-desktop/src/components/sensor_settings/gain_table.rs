pub(super) struct GainEntry {
    pub(super) label: &'static str,
    pub(super) b_global: u8,
    pub(super) b_ggh: u8,
}

/// Decode supported gain settings, including the equivalent 2x/4x boundary
/// encodings with `b_global = 16`. Never turn an unknown register value into a
/// different gain that would be silently written back on save.
pub(super) fn gain_index_from_reg(b_global: u8, b_ggh: u8) -> Option<usize> {
    let group = match b_ggh {
        0 | 1 => 0,
        2 => 1,
        3 => 2,
        _ => return None,
    };
    if b_global > 16 {
        return None;
    }
    Some(group * 16 + usize::from(b_global))
}

pub(super) static GAIN_TABLE: &[GainEntry] = &[
    // 1x multiplier (b_ggh=0)
    GainEntry {
        label: "1.0000",
        b_global: 0,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.0625",
        b_global: 1,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.1250",
        b_global: 2,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.1875",
        b_global: 3,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.2500",
        b_global: 4,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.3125",
        b_global: 5,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.3750",
        b_global: 6,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.4375",
        b_global: 7,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.5000",
        b_global: 8,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.5625",
        b_global: 9,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.6250",
        b_global: 10,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.6875",
        b_global: 11,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.7500",
        b_global: 12,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.8125",
        b_global: 13,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.8750",
        b_global: 14,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.9375",
        b_global: 15,
        b_ggh: 0,
    },
    // 2x multiplier (b_ggh=2)
    GainEntry {
        label: "2.0000",
        b_global: 0,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.1250",
        b_global: 1,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.2500",
        b_global: 2,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.3750",
        b_global: 3,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.5000",
        b_global: 4,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.6250",
        b_global: 5,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.7500",
        b_global: 6,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.8750",
        b_global: 7,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.0000",
        b_global: 8,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.1250",
        b_global: 9,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.2500",
        b_global: 10,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.3750",
        b_global: 11,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.5000",
        b_global: 12,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.6250",
        b_global: 13,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.7500",
        b_global: 14,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.8750",
        b_global: 15,
        b_ggh: 2,
    },
    // 4x multiplier (b_ggh=3)
    GainEntry {
        label: "4.0000",
        b_global: 0,
        b_ggh: 3,
    },
    GainEntry {
        label: "4.2500",
        b_global: 1,
        b_ggh: 3,
    },
    GainEntry {
        label: "4.5000",
        b_global: 2,
        b_ggh: 3,
    },
    GainEntry {
        label: "4.7500",
        b_global: 3,
        b_ggh: 3,
    },
    GainEntry {
        label: "5.0000",
        b_global: 4,
        b_ggh: 3,
    },
    GainEntry {
        label: "5.2500",
        b_global: 5,
        b_ggh: 3,
    },
    GainEntry {
        label: "5.5000",
        b_global: 6,
        b_ggh: 3,
    },
    GainEntry {
        label: "5.7500",
        b_global: 7,
        b_ggh: 3,
    },
    GainEntry {
        label: "6.0000",
        b_global: 8,
        b_ggh: 3,
    },
    GainEntry {
        label: "6.2500",
        b_global: 9,
        b_ggh: 3,
    },
    GainEntry {
        label: "6.5000",
        b_global: 10,
        b_ggh: 3,
    },
    GainEntry {
        label: "6.7500",
        b_global: 11,
        b_ggh: 3,
    },
    GainEntry {
        label: "7.0000",
        b_global: 12,
        b_ggh: 3,
    },
    GainEntry {
        label: "7.2500",
        b_global: 13,
        b_ggh: 3,
    },
    GainEntry {
        label: "7.5000",
        b_global: 14,
        b_ggh: 3,
    },
    GainEntry {
        label: "7.7500",
        b_global: 15,
        b_ggh: 3,
    },
    GainEntry {
        label: "8.0000",
        b_global: 16,
        b_ggh: 3,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_gains_and_boundary_aliases_decode_without_clamping() {
        for (index, entry) in GAIN_TABLE.iter().enumerate() {
            assert_eq!(
                gain_index_from_reg(entry.b_global, entry.b_ggh),
                Some(index)
            );
        }
        assert_eq!(gain_index_from_reg(16, 0), Some(16));
        assert_eq!(gain_index_from_reg(16, 2), Some(32));
        assert_eq!(gain_index_from_reg(17, 3), None);
        assert_eq!(gain_index_from_reg(255, 0), None);
        assert_eq!(gain_index_from_reg(0, 1), Some(0));
        assert_eq!(gain_index_from_reg(0, 4), None);
    }
}
