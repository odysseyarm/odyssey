//! Where a tracker points over time: aim paths, hand handling and button events.
//! Every output (live simulator, replay, Blender scene) samples the same functions.

use serde::{Deserialize, Serialize};

use crate::StageError;

/// Aim stays this far from the display edge so every sample hits the display.
const EDGE: f64 = 0.02;

/// A path over the display in display fractions, `[0, 1]` on both axes.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Pattern {
    /// Since: Odyssey Desktop 0.7.5.
    Hold {
        /// Since: Odyssey Desktop 0.7.5.
        at: [f64; 2],
    },
    /// Since: Odyssey Desktop 0.7.5.
    Circle {
        /// Since: Odyssey Desktop 0.7.5.
        center: [f64; 2],
        /// Since: Odyssey Desktop 0.7.5.
        radius: [f64; 2],
        /// Since: Odyssey Desktop 0.7.5.
        period_s: f64,
    },
    /// `frequency` counts cycles per period on each axis. `[1, 2]` draws a figure eight.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    Lissajous {
        /// Since: Odyssey Desktop 0.7.5.
        center: [f64; 2],
        /// Since: Odyssey Desktop 0.7.5.
        amplitude: [f64; 2],
        /// Since: Odyssey Desktop 0.7.5.
        frequency: [f64; 2],
        /// Since: Odyssey Desktop 0.7.5.
        phase_deg: f64,
        /// Since: Odyssey Desktop 0.7.5.
        period_s: f64,
    },
    /// Back-and-forth sweep over `rows` lines.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    Raster {
        /// Since: Odyssey Desktop 0.7.5.
        rows: u32,
        /// Since: Odyssey Desktop 0.7.5.
        margin: f64,
        /// Since: Odyssey Desktop 0.7.5.
        period_s: f64,
    },
    /// Visit points in order along straight lines, then return to the first.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    Tour {
        /// Since: Odyssey Desktop 0.7.5.
        points: Vec<[f64; 2]>,
        /// Since: Odyssey Desktop 0.7.5.
        period_s: f64,
    },
}

/// Since: Odyssey Desktop 0.7.5.
pub const PATTERN_NAMES: [&str; 6] = [
    "hold",
    "circle",
    "figure-eight",
    "raster",
    "corners",
    "nine-point",
];

impl Pattern {
    /// Since: Odyssey Desktop 0.7.5.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "hold" => Self::Hold { at: [0.5, 0.5] },
            "circle" => Self::Circle {
                center: [0.5, 0.5],
                radius: [0.3, 0.3],
                period_s: 6.0,
            },
            "figure-eight" => Self::Lissajous {
                center: [0.5, 0.5],
                amplitude: [0.35, 0.3],
                frequency: [1.0, 2.0],
                phase_deg: 90.0,
                period_s: 8.0,
            },
            "raster" => Self::Raster {
                rows: 5,
                margin: 0.1,
                period_s: 12.0,
            },
            "corners" => Self::Tour {
                points: vec![[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]],
                period_s: 8.0,
            },
            "nine-point" => Self::Tour {
                points: vec![
                    [0.1, 0.1],
                    [0.5, 0.1],
                    [0.9, 0.1],
                    [0.9, 0.5],
                    [0.5, 0.5],
                    [0.1, 0.5],
                    [0.1, 0.9],
                    [0.5, 0.9],
                    [0.9, 0.9],
                ],
                period_s: 14.0,
            },
            _ => return None,
        })
    }

    /// # Errors
    ///
    /// Returns an error if any aim-pattern parameter is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn validate(&self) -> Result<(), StageError> {
        let finite = |values: &[f64]| values.iter().all(|value| value.is_finite());
        let ok = match self {
            Self::Hold { at } => finite(at),
            Self::Circle {
                center,
                radius,
                period_s,
            } => finite(center) && finite(radius) && *period_s > 0.0 && period_s.is_finite(),
            Self::Lissajous {
                center,
                amplitude,
                frequency,
                phase_deg,
                period_s,
            } => {
                finite(center)
                    && finite(amplitude)
                    && finite(frequency)
                    && phase_deg.is_finite()
                    && *period_s > 0.0
                    && period_s.is_finite()
            }
            Self::Raster {
                rows,
                margin,
                period_s,
            } => {
                (1..=64).contains(rows)
                    && margin.is_finite()
                    && *period_s > 0.0
                    && period_s.is_finite()
            }
            Self::Tour { points, period_s } => {
                (1..=256).contains(&points.len())
                    && points.iter().all(|point| finite(point))
                    && *period_s > 0.0
                    && period_s.is_finite()
            }
        };
        if ok {
            Ok(())
        } else {
            Err(StageError::invalid("invalid aim pattern"))
        }
    }

    /// Aim at `t_s` seconds, kept inside the display.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn uv(&self, t_s: f64) -> [f64; 2] {
        let raw = match self {
            Self::Hold { at } => *at,
            Self::Circle {
                center,
                radius,
                period_s,
            } => {
                let angle = std::f64::consts::TAU * t_s / period_s;
                [
                    center[0] + radius[0] * angle.cos(),
                    center[1] + radius[1] * angle.sin(),
                ]
            }
            Self::Lissajous {
                center,
                amplitude,
                frequency,
                phase_deg,
                period_s,
            } => {
                let angle = std::f64::consts::TAU * t_s / period_s;
                [
                    center[0]
                        + amplitude[0] * (frequency[0] * angle + phase_deg.to_radians()).sin(),
                    center[1] + amplitude[1] * (frequency[1] * angle).sin(),
                ]
            }
            Self::Raster {
                rows,
                margin,
                period_s,
            } => {
                let lines = *rows;
                let mut points = Vec::with_capacity(2 * lines as usize);
                for row in 0..lines {
                    let v = if lines == 1 {
                        0.5
                    } else {
                        margin + (1.0 - 2.0 * margin) * f64::from(row) / f64::from(lines - 1)
                    };
                    let (left, right) = (*margin, 1.0 - margin);
                    if row % 2 == 0 {
                        points.extend([[left, v], [right, v]]);
                    } else {
                        points.extend([[right, v], [left, v]]);
                    }
                }
                along(&points, t_s / period_s, false)
            }
            Self::Tour { points, period_s } => along(points, t_s / period_s, true),
        };
        [
            raw[0].clamp(EDGE, 1.0 - EDGE),
            raw[1].clamp(EDGE, 1.0 - EDGE),
        ]
    }
}

/// Point at fraction `s` (wrapping) of the total length of a polyline.
fn along(points: &[[f64; 2]], s: f64, closed: bool) -> [f64; 2] {
    let Some(&first) = points.first() else {
        return [0.5, 0.5];
    };
    let mut legs: Vec<([f64; 2], [f64; 2])> = points.windows(2).map(|w| (w[0], w[1])).collect();
    if closed && let Some(&last) = points.last() {
        legs.push((last, first));
    }
    let length = |leg: &([f64; 2], [f64; 2])| (leg.1[0] - leg.0[0]).hypot(leg.1[1] - leg.0[1]);
    let total: f64 = legs.iter().map(length).sum();
    if total <= 0.0 {
        return first;
    }
    let mut remaining = s.rem_euclid(1.0) * total;
    for leg in &legs {
        let leg_length = length(leg);
        if remaining <= leg_length && leg_length > 0.0 {
            let f = remaining / leg_length;
            return [
                leg.0[0] + (leg.1[0] - leg.0[0]) * f,
                leg.0[1] + (leg.1[1] - leg.0[1]) * f,
            ];
        }
        remaining -= leg_length;
    }
    first
}

/// How the shooter's body moves the tracker. All angles are degrees. The phases
/// derive from `seed`, so a run repeats exactly.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Handling {
    /// Physiological tremor amplitude. Real hands shake around 8–12 Hz.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub tremor_deg: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub tremor_hz: f64,
    /// Slow wander of the aim.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub drift_deg: f64,
    /// Upward kick on each trigger pull, decaying within a fraction of a second.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub recoil_deg: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub seed: u64,
}

impl Default for Handling {
    fn default() -> Self {
        Self {
            tremor_deg: 0.0,
            tremor_hz: 9.0,
            drift_deg: 0.0,
            recoil_deg: 0.0,
            seed: 1,
        }
    }
}

const RECOIL_DECAY_S: f64 = 0.12;

impl Handling {
    /// # Errors
    ///
    /// Returns an error if any handling parameter is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn validate(&self) -> Result<(), StageError> {
        let ok = [
            self.tremor_deg,
            self.tremor_hz,
            self.drift_deg,
            self.recoil_deg,
        ]
        .iter()
        .all(|value| value.is_finite() && *value >= 0.0)
            && self.tremor_deg <= 10.0
            && self.drift_deg <= 20.0
            && self.recoil_deg <= 20.0
            && self.tremor_hz <= 60.0;
        if ok {
            Ok(())
        } else {
            Err(StageError::invalid("invalid hand handling"))
        }
    }

    /// Yaw and pitch offsets in radians at `t_s`. Positive pitch aims up.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn angles(&self, t_s: f64, triggers: &[f64]) -> (f64, f64) {
        let phase = |index: u64| {
            let mut x = self
                .seed
                .wrapping_add(index.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^= x >> 31;
            (x >> 11) as f64 / (1_u64 << 53) as f64 * std::f64::consts::TAU
        };
        let wave = |hz: f64, index: u64| (std::f64::consts::TAU * hz * t_s + phase(index)).sin();
        let tremor = |axis: u64| {
            self.tremor_deg
                * (wave(self.tremor_hz, axis) + 0.5 * wave(self.tremor_hz * 1.7, axis + 2))
                / 1.5
        };
        let drift =
            |axis: u64| self.drift_deg * (wave(0.13, axis + 4) + 0.6 * wave(0.31, axis + 6)) / 1.6;
        let kick: f64 = triggers
            .iter()
            .filter(|at| **at <= t_s)
            .map(|at| (-(t_s - at) / RECOIL_DECAY_S).exp())
            .sum();
        (
            (tremor(0) + drift(0)).to_radians(),
            (tremor(1) + drift(1) + self.recoil_deg * kick).to_radians(),
        )
    }
}

/// Where the tracker is, apart from where it points.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Movement {
    /// Since: Odyssey Desktop 0.7.5.
    #[default]
    Still,
    /// Glides between the device's position and `to`, and back.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    Walk {
        #[doc = "Since: Odyssey Desktop 0.7.5."]
        to: [f64; 3],
        #[doc = "Since: Odyssey Desktop 0.7.5."]
        period_s: f64,
    },
    /// Circles the device's position in the floor plane.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    Orbit {
        #[doc = "Since: Odyssey Desktop 0.7.5."]
        radius_m: f64,
        #[doc = "Since: Odyssey Desktop 0.7.5."]
        period_s: f64,
    },
}

impl Movement {
    /// # Errors
    ///
    /// Returns an error if movement parameters are invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn validate(&self) -> Result<(), StageError> {
        let ok = match self {
            Self::Still => true,
            Self::Walk { to, period_s } => {
                to.iter().all(|value| value.is_finite()) && *period_s > 0.0 && period_s.is_finite()
            }
            Self::Orbit { radius_m, period_s } => {
                radius_m.is_finite() && *radius_m >= 0.0 && *period_s > 0.0 && period_s.is_finite()
            }
        };
        if ok {
            Ok(())
        } else {
            Err(StageError::invalid("invalid movement"))
        }
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn position(&self, home: [f64; 3], t_s: f64) -> [f64; 3] {
        match self {
            Self::Still => home,
            Self::Walk { to, period_s } => {
                let f = 0.5 - 0.5 * (std::f64::consts::TAU * t_s / period_s).cos();
                std::array::from_fn(|i| home[i] + (to[i] - home[i]) * f)
            }
            Self::Orbit { radius_m, period_s } => {
                let angle = std::f64::consts::TAU * t_s / period_s;
                [
                    home[0] + radius_m * angle.cos(),
                    home[1],
                    home[2] + radius_m * angle.sin(),
                ]
            }
        }
    }
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    /// Since: Odyssey Desktop 0.7.5.
    Trigger,
    /// Since: Odyssey Desktop 0.7.5.
    Pair,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Press {
    /// Since: Odyssey Desktop 0.7.5.
    pub at_s: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub button: Button,
}

/// Trigger pull times in seconds, in ascending order.
///
/// Since: Odyssey Desktop 0.7.5.
pub fn trigger_times(presses: &[Press]) -> Vec<f64> {
    let mut times: Vec<f64> = presses
        .iter()
        .filter(|press| press.button == Button::Trigger)
        .map(|press| press.at_s)
        .collect();
    times.sort_by(f64::total_cmp);
    times
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_pattern_stays_on_the_display() {
        for name in [
            "hold",
            "circle",
            "figure-eight",
            "raster",
            "corners",
            "nine-point",
        ] {
            let pattern = Pattern::named(name).unwrap();
            pattern.validate().unwrap();
            for step in 0..400 {
                let [u, v] = pattern.uv(f64::from(step) * 0.07);
                assert!(
                    (EDGE..=1.0 - EDGE).contains(&u) && (EDGE..=1.0 - EDGE).contains(&v),
                    "{name}: {u},{v}"
                );
            }
        }
    }

    #[test]
    fn tour_returns_to_its_start_each_period() {
        let pattern = Pattern::named("corners").unwrap();
        let start = pattern.uv(0.0);
        let again = pattern.uv(8.0);
        assert!((start[0] - again[0]).abs() < 1e-9 && (start[1] - again[1]).abs() < 1e-9);
    }

    #[test]
    fn raster_sweeps_alternate_direction() {
        let pattern = Pattern::Raster {
            rows: 2,
            margin: 0.1,
            period_s: 10.0,
        };
        // Two lines of 0.8 plus the 0.8 connector make a path of 2.4: the first
        // line ends a third of the way in.
        let end_of_first = pattern.uv(10.0 / 3.0);
        assert!((end_of_first[0] - 0.9).abs() < 1e-9, "{end_of_first:?}");
        let end_of_second = pattern.uv(10.0 * 0.999);
        assert!((end_of_second[0] - 0.1).abs() < 0.01, "{end_of_second:?}");
    }

    #[test]
    fn handling_repeats_exactly_and_differs_by_seed() {
        let handling = Handling {
            tremor_deg: 0.3,
            drift_deg: 1.0,
            ..Handling::default()
        };
        assert_eq!(handling.angles(1.234, &[]), handling.angles(1.234, &[]));
        let other = Handling {
            seed: 2,
            ..handling
        };
        assert_ne!(handling.angles(1.234, &[]), other.angles(1.234, &[]));
        let (yaw, pitch) = handling.angles(3.0, &[]);
        assert!(yaw.abs() < 2.0_f64.to_radians() && pitch.abs() < 2.0_f64.to_radians());
    }

    #[test]
    fn recoil_kicks_up_then_settles() {
        let handling = Handling {
            recoil_deg: 2.0,
            ..Handling::default()
        };
        let at_shot = handling.angles(1.0, &[1.0]).1.to_degrees();
        let later = handling.angles(2.0, &[1.0]).1.to_degrees();
        let before = handling.angles(0.5, &[1.0]).1;
        assert!((at_shot - 2.0).abs() < 1e-9 && later < 1e-3 && before == 0.0);
    }

    #[test]
    fn walk_goes_out_and_back() {
        let movement = Movement::Walk {
            to: [1.0, 0.0, -3.0],
            period_s: 10.0,
        };
        let home = [0.0, 0.0, -2.0];
        assert_eq!(movement.position(home, 0.0), home);
        let far = movement.position(home, 5.0);
        assert!((far[0] - 1.0).abs() < 1e-9 && (far[2] + 3.0).abs() < 1e-9);
    }
}
