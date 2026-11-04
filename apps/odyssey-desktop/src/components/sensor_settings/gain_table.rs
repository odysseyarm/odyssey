pub(super) struct GainEntry {
    pub(super) label: &'static str,
    pub(super) b_global: u8,
    pub(super) b_ggh: u8,
}

pub(super) fn gain_index_from_reg(b_global: u8, mut b_ggh: u8) -> usize {
    b_ggh = b_ggh.saturating_sub(1);
    (b_ggh as usize) * 16 + (b_global as usize)
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
