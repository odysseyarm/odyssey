//! The green/amber/red verdict on a finished measurement.
//!
//! Three independent checks each grade as good, fair or poor; the verdict is
//! the worst of them. The thresholds are engineering judgment, not a
//! specification: they sit inside what the solver itself accepts so that a
//! result the solver produced is never flagged for something it cannot fix.
//!
//! | Check                 | Good           | Fair            | Poor      |
//! |-----------------------|----------------|-----------------|-----------|
//! | Plane fit RMSE        | up to 5 mm     | up to 10 mm     | above     |
//! | Dots used             | 90 % and up    | 70 % and up     | below     |
//! | Marker offset         | up to 3 % diag | up to 6 % diag  | above     |
//!
//! - *Plane fit RMSE* is the depth noise left after fitting the screen plane.
//!   The live preview already refuses 15 mm; 5 mm is typical of a camera at
//!   2 m on a well lit screen.
//! - *Dots used* is the share of the pattern's dots that agreed with a flat
//!   screen. A few misses from glare are normal; a large loss means the
//!   pattern was partly hidden.
//! - *Marker offset* is the largest distance between a measured marker and
//!   its place in the tracker layout, as a share of the screen diagonal. The
//!   solver matches markers within 10 % of the diagonal; 3 % (about 4 cm on a
//!   55 inch screen) is mounting tolerance, 6 % means a marker is probably on
//!   the wrong spot or the screen is not the one the layout was made for.

use odyssey_autocalib::session::Review;

pub const RMSE_GOOD_M: f64 = 0.005;
pub const RMSE_FAIR_M: f64 = 0.010;
pub const DOTS_GOOD: f64 = 0.90;
pub const DOTS_FAIR: f64 = 0.70;
pub const OFFSET_GOOD_OF_DIAGONAL: f64 = 0.03;
pub const OFFSET_FAIR_OF_DIAGONAL: f64 = 0.06;

/// Ordered worst last, so `max` picks the overall verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    Good,
    Fair,
    Poor,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub label: &'static str,
    /// The measured value, formatted for the Details list.
    pub value: String,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quality {
    pub verdict: Verdict,
    pub checks: [Check; 3],
}

/// The numbers a verdict is computed from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measures {
    pub plane_rmse_m: f64,
    pub dots_used: usize,
    pub dots_expected: usize,
    pub marker_max_offset_m: f64,
    pub diagonal_m: f64,
}

impl From<&Review> for Measures {
    fn from(r: &Review) -> Self {
        Self {
            plane_rmse_m: r.plane_rmse_m,
            dots_used: r.dots_used,
            dots_expected: r.dots_expected,
            marker_max_offset_m: r.marker_max_offset_m,
            diagonal_m: r.size_m[0].hypot(r.size_m[1]),
        }
    }
}

/// Grades `value` against ascending limits where lower is better.
fn grade_low(value: f64, good: f64, fair: f64) -> Verdict {
    // NaN fails both comparisons and lands on Poor, which is the safe side.
    if value <= good {
        Verdict::Good
    } else if value <= fair {
        Verdict::Fair
    } else {
        Verdict::Poor
    }
}

pub fn assess(m: &Measures) -> Quality {
    let dots_share = if m.dots_expected == 0 { 0.0 } else { m.dots_used as f64 / m.dots_expected as f64 };
    let offset_share = if m.diagonal_m > 0.0 { m.marker_max_offset_m / m.diagonal_m } else { f64::INFINITY };

    let checks = [
        Check {
            label: "Depth accuracy",
            value: format!("{:.1} mm", m.plane_rmse_m * 1000.0),
            verdict: grade_low(m.plane_rmse_m, RMSE_GOOD_M, RMSE_FAIR_M),
        },
        Check {
            label: "Dots recognised",
            value: format!("{} of {}", m.dots_used, m.dots_expected),
            // Higher is better, so grade the shortfall.
            verdict: grade_low(1.0 - dots_share, 1.0 - DOTS_GOOD, 1.0 - DOTS_FAIR),
        },
        Check {
            label: "Marker placement",
            value: format!("{:.1} cm off layout", m.marker_max_offset_m * 100.0),
            verdict: grade_low(offset_share, OFFSET_GOOD_OF_DIAGONAL, OFFSET_FAIR_OF_DIAGONAL),
        },
    ];
    let verdict = checks.iter().map(|c| c.verdict).max().unwrap_or(Verdict::Poor);
    Quality { verdict, checks }
}

impl Quality {
    /// The worst check, which the advice is about.
    pub fn weakest(&self) -> &Check {
        self.checks.iter().max_by_key(|c| c.verdict).unwrap_or(&self.checks[0])
    }

    /// What to do next, in words for the person holding the camera.
    pub fn advice(&self) -> &'static str {
        match self.verdict {
            Verdict::Good => "The measurement is accurate. Save it to start using this screen.",
            Verdict::Fair => match self.weakest().label {
                "Depth accuracy" => "Usable. For a sharper result, redo it with the camera a little closer and away from reflections.",
                "Dots recognised" => "Usable. For a sharper result, redo it with the whole screen in view and less glare.",
                _ => "Usable. If the markers were moved, check they sit where the layout shows, then redo.",
            },
            Verdict::Poor => match self.weakest().label {
                "Depth accuracy" => "The depth reading was too noisy. Redo it with the camera closer, steady, and away from direct sunlight and reflections.",
                "Dots recognised" => "Too many dots were hidden. Redo it with the whole screen in view and no glare on it.",
                _ => "The markers are not where they should be on this screen. Check they are mounted on the right screen and switched on, then redo.",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Measures {
        Measures {
            plane_rmse_m: 0.003,
            dots_used: 54,
            dots_expected: 54,
            marker_max_offset_m: 0.01,
            diagonal_m: 1.4,
        }
    }

    #[test]
    fn clean_measurement_is_good() {
        assert_eq!(assess(&good()).verdict, Verdict::Good);
    }

    #[test]
    fn thresholds_are_inclusive_at_the_boundary() {
        let at = |m: Measures| assess(&m).verdict;
        assert_eq!(at(Measures { plane_rmse_m: RMSE_GOOD_M, ..good() }), Verdict::Good);
        assert_eq!(at(Measures { plane_rmse_m: RMSE_GOOD_M + 1e-6, ..good() }), Verdict::Fair);
        assert_eq!(at(Measures { plane_rmse_m: RMSE_FAIR_M, ..good() }), Verdict::Fair);
        assert_eq!(at(Measures { plane_rmse_m: RMSE_FAIR_M + 1e-6, ..good() }), Verdict::Poor);
    }

    #[test]
    fn dot_share_grades_both_ways() {
        let with = |used| assess(&Measures { dots_used: used, dots_expected: 100, ..good() }).verdict;
        assert_eq!(with(100), Verdict::Good);
        assert_eq!(with(90), Verdict::Good);
        assert_eq!(with(89), Verdict::Fair);
        assert_eq!(with(70), Verdict::Fair);
        assert_eq!(with(69), Verdict::Poor);
        assert_eq!(with(0), Verdict::Poor);
    }

    #[test]
    fn marker_offset_scales_with_the_screen() {
        let m = Measures { marker_max_offset_m: 0.04, ..good() };
        // 4 cm is within tolerance on a 55 inch screen but not on a 20 inch one.
        assert_eq!(assess(&Measures { diagonal_m: 1.4, ..m }).verdict, Verdict::Good);
        assert_eq!(assess(&Measures { diagonal_m: 0.5, ..m }).verdict, Verdict::Poor);
    }

    #[test]
    fn the_worst_check_decides() {
        let q = assess(&Measures { plane_rmse_m: 0.008, marker_max_offset_m: 0.2, ..good() });
        assert_eq!(q.verdict, Verdict::Poor);
        assert_eq!(q.weakest().label, "Marker placement");
        assert!(q.advice().contains("markers"));
    }

    #[test]
    fn degenerate_inputs_are_poor_not_a_panic() {
        assert_eq!(assess(&Measures { dots_expected: 0, dots_used: 0, ..good() }).verdict, Verdict::Poor);
        assert_eq!(assess(&Measures { plane_rmse_m: f64::NAN, ..good() }).verdict, Verdict::Poor);
        assert_eq!(assess(&Measures { diagonal_m: 0.0, ..good() }).verdict, Verdict::Poor);
    }
}
