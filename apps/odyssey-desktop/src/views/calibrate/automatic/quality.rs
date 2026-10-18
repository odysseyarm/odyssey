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
//! | Size agreement        | within 2 %     | any larger gap  | never     |
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
//! - *Size agreement* compares the depth camera's measurement with the size
//!   the display or the tech declared. Depth scale error stays under 2 % at
//!   room distances, so a bigger gap means the display crops its picture or
//!   the declared size is wrong. Either way the result is amber, never green:
//!   it is not clear which number is right, and the tech decides.

use odyssey_autocalib::session::Review;
use odyssey_autocalib::solve::{SIZE_DISAGREEMENT, SizeCheck};

use super::kind::SizeOrigin;

/// Label of the size-agreement check, which the review also looks up.
/// What to tell the tech when the camera and the declared size disagree: how
/// far apart they are, and why that might be.
pub fn disagreement_message(check: &SizeCheck, origin: Option<SizeOrigin>) -> String {
    let ratio = (check.ratio_w + check.ratio_h) / 2.0;
    let percent = ((ratio - 1.0).abs() * 100.0).round();
    let how_much = if percent < 1.0 {
        "a little".to_string()
    } else {
        format!("about {percent:.0}%")
    };
    let direction = if ratio < 1.0 { "smaller" } else { "larger" };
    match origin {
        Some(SizeOrigin::Entered) => format!(
            "The camera measured the screen {how_much} {direction} than the size you entered. The size may be wrong, or the TV may be cropping the picture."
        ),
        _ => format!(
            "The camera measured the screen {how_much} {direction} than the TV says. The TV may be cropping the picture, or the size is wrong."
        ),
    }
}

pub const SIZE_LABEL: &str = "Size agreement";

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
    pub checks: Vec<Check>,
}

/// The numbers a verdict is computed from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measures {
    pub plane_rmse_m: f64,
    pub dots_used: usize,
    pub dots_expected: usize,
    pub marker_max_offset_m: f64,
    pub diagonal_m: f64,
    /// The larger relative gap between the depth measurement and the declared
    /// size along either axis; present only when a size was declared.
    pub size_disagreement: Option<f64>,
}

impl From<&Review> for Measures {
    fn from(r: &Review) -> Self {
        Self {
            plane_rmse_m: r.plane_rmse_m,
            dots_used: r.dots_used,
            dots_expected: r.dots_expected,
            marker_max_offset_m: r.marker_max_offset_m,
            diagonal_m: r.size_m[0].hypot(r.size_m[1]),
            size_disagreement: r
                .size_check
                .map(|c| (c.ratio_w - 1.0).abs().max((c.ratio_h - 1.0).abs())),
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
    let dots_share = if m.dots_expected == 0 {
        0.0
    } else {
        m.dots_used as f64 / m.dots_expected as f64
    };
    let offset_share = if m.diagonal_m > 0.0 {
        m.marker_max_offset_m / m.diagonal_m
    } else {
        f64::INFINITY
    };

    let mut checks = vec![
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
            verdict: grade_low(
                offset_share,
                OFFSET_GOOD_OF_DIAGONAL,
                OFFSET_FAIR_OF_DIAGONAL,
            ),
        },
    ];
    if let Some(gap) = m.size_disagreement {
        checks.push(Check {
            label: SIZE_LABEL,
            value: format!("{:.1} % apart", gap * 100.0),
            // Either number could be the wrong one, so a gap is a caution and
            // never a failure.
            verdict: if gap <= SIZE_DISAGREEMENT {
                Verdict::Good
            } else {
                Verdict::Fair
            },
        });
    }
    let verdict = checks
        .iter()
        .map(|c| c.verdict)
        .max()
        .unwrap_or(Verdict::Poor);
    Quality { verdict, checks }
}

impl Quality {
    /// The worst check, which the advice is about.
    pub fn weakest(&self) -> &Check {
        // `max_by_key` keeps the last of equal checks, so scan for the first
        // worst one to keep the advice stable.
        let worst = self
            .checks
            .iter()
            .map(|c| c.verdict)
            .max()
            .unwrap_or(Verdict::Good);
        self.checks
            .iter()
            .find(|c| c.verdict == worst)
            .unwrap_or(&self.checks[0])
    }

    /// What to do next, in words for the person holding the camera.
    pub fn advice(&self) -> &'static str {
        match self.verdict {
            Verdict::Good => "The measurement is accurate. Save it to start using this screen.",
            Verdict::Fair => match self.weakest().label {
                "Depth accuracy" => {
                    "Usable. For a sharper result, redo it with the camera a little closer and away from reflections."
                }
                SIZE_LABEL => {
                    "The camera and the declared size don't agree. See below before saving."
                }
                "Dots recognised" => {
                    "Usable. For a sharper result, redo it with the whole screen in view and less glare."
                }
                _ => {
                    "Usable. If the markers were moved, check they sit where the layout shows, then redo."
                }
            },
            Verdict::Poor => match self.weakest().label {
                "Depth accuracy" => {
                    "The depth reading was too noisy. Redo it with the camera closer, steady, and away from direct sunlight and reflections."
                }
                "Dots recognised" => {
                    "Too many dots were hidden. Redo it with the whole screen in view and no glare on it."
                }
                _ => {
                    "The markers are not where they should be on this screen. Check they are mounted on the right screen and switched on, then redo."
                }
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
            size_disagreement: None,
        }
    }

    #[test]
    fn clean_measurement_is_good() {
        assert_eq!(assess(&good()).verdict, Verdict::Good);
    }

    #[test]
    fn thresholds_are_inclusive_at_the_boundary() {
        let at = |m: Measures| assess(&m).verdict;
        assert_eq!(
            at(Measures {
                plane_rmse_m: RMSE_GOOD_M,
                ..good()
            }),
            Verdict::Good
        );
        assert_eq!(
            at(Measures {
                plane_rmse_m: RMSE_GOOD_M + 1e-6,
                ..good()
            }),
            Verdict::Fair
        );
        assert_eq!(
            at(Measures {
                plane_rmse_m: RMSE_FAIR_M,
                ..good()
            }),
            Verdict::Fair
        );
        assert_eq!(
            at(Measures {
                plane_rmse_m: RMSE_FAIR_M + 1e-6,
                ..good()
            }),
            Verdict::Poor
        );
    }

    #[test]
    fn dot_share_grades_both_ways() {
        let with = |used| {
            assess(&Measures {
                dots_used: used,
                dots_expected: 100,
                ..good()
            })
            .verdict
        };
        assert_eq!(with(100), Verdict::Good);
        assert_eq!(with(90), Verdict::Good);
        assert_eq!(with(89), Verdict::Fair);
        assert_eq!(with(70), Verdict::Fair);
        assert_eq!(with(69), Verdict::Poor);
        assert_eq!(with(0), Verdict::Poor);
    }

    #[test]
    fn marker_offset_scales_with_the_screen() {
        let m = Measures {
            marker_max_offset_m: 0.04,
            ..good()
        };
        // 4 cm is within tolerance on a 55 inch screen but not on a 20 inch one.
        assert_eq!(
            assess(&Measures {
                diagonal_m: 1.4,
                ..m
            })
            .verdict,
            Verdict::Good
        );
        assert_eq!(
            assess(&Measures {
                diagonal_m: 0.5,
                ..m
            })
            .verdict,
            Verdict::Poor
        );
    }

    #[test]
    fn the_worst_check_decides() {
        let q = assess(&Measures {
            plane_rmse_m: 0.008,
            marker_max_offset_m: 0.2,
            ..good()
        });
        assert_eq!(q.verdict, Verdict::Poor);
        assert_eq!(q.weakest().label, "Marker placement");
        assert!(q.advice().contains("markers"));
    }

    #[test]
    fn a_size_that_agrees_keeps_the_verdict_green() {
        let q = assess(&Measures {
            size_disagreement: Some(0.01),
            ..good()
        });
        assert_eq!(q.verdict, Verdict::Good);
        assert_eq!(q.checks.len(), 4);
        assert_eq!(assess(&good()).checks.len(), 3);
        assert_eq!(
            assess(&Measures {
                size_disagreement: Some(SIZE_DISAGREEMENT),
                ..good()
            })
            .verdict,
            Verdict::Good
        );
    }

    #[test]
    fn a_size_that_disagrees_is_amber_never_green_or_red() {
        for gap in [0.021, 0.05, 0.4] {
            let q = assess(&Measures {
                size_disagreement: Some(gap),
                ..good()
            });
            assert_eq!(q.verdict, Verdict::Fair, "{gap}");
            assert_eq!(q.weakest().label, SIZE_LABEL);
            assert!(q.advice().contains("agree"));
        }
    }

    #[test]
    fn the_disagreement_says_which_way_and_how_far() {
        let smaller = SizeCheck::new([1.15, 0.65], [1.21, 0.68]);
        let text = disagreement_message(&smaller, Some(SizeOrigin::Panel));
        assert!(text.contains("about 5% smaller than the TV says"), "{text}");
        assert!(text.contains("cropping"));
        let larger = SizeCheck::new([1.0, 0.56], [0.71, 0.4]);
        assert!(
            disagreement_message(&larger, Some(SizeOrigin::Entered))
                .contains("larger than the size you entered")
        );
        let tiny = SizeCheck::new([1.215, 0.683], [1.21, 0.68]);
        assert!(disagreement_message(&tiny, None).contains("a little"));
    }

    #[test]
    fn degenerate_inputs_are_poor_not_a_panic() {
        assert_eq!(
            assess(&Measures {
                dots_expected: 0,
                dots_used: 0,
                ..good()
            })
            .verdict,
            Verdict::Poor
        );
        assert_eq!(
            assess(&Measures {
                plane_rmse_m: f64::NAN,
                ..good()
            })
            .verdict,
            Verdict::Poor
        );
        assert_eq!(
            assess(&Measures {
                diagonal_m: 0.0,
                ..good()
            })
            .verdict,
            Verdict::Poor
        );
    }
}
