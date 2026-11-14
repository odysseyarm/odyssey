//! Radio link model: log-distance path loss with obstructions and slow fading.
//!
//! The defaults come from the nRF52840 datasheet (LE 1M receive sensitivity near
//! −95 dBm, transmit up to +8 dBm) and the free-space loss at 2.44 GHz. The
//! exponent and wall losses are rule-of-thumb values for indoor rooms, not
//! measurements of Odyssey hardware, so treat ranges as relative comparisons.

use serde::{Deserialize, Serialize};

use crate::StageError;

/// Free-space loss at 1 m and 2.44 GHz: 20·log10(4π·1 m / λ), λ = 12.3 cm.
const LOSS_AT_1M_DB: f64 = 40.2;
const MIN_DISTANCE_M: f64 = 0.1;
/// Rms of 0.7·sin + 0.5·sin: sqrt((0.7² + 0.5²) / 2).
const FADE_RMS: f64 = 0.608;

/// Typical path-loss exponents by room type, for quick comparisons.
///
/// Since: Odyssey Desktop 0.7.5.
pub const ENVIRONMENTS: [(&str, f64); 3] = [("open", 2.0), ("furnished", 3.0), ("cluttered", 3.8)];

/// A flat obstruction in the floor plan, from `a` to `b` as `[x, z]` points.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wall {
    /// Since: Odyssey Desktop 0.7.5.
    pub a: [f64; 2],
    /// Since: Odyssey Desktop 0.7.5.
    pub b: [f64; 2],
    /// Since: Odyssey Desktop 0.7.5.
    pub loss_db: f64,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Radio {
    /// Since: Odyssey Desktop 0.7.5.
    pub tx_power_dbm: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub sensitivity_dbm: f64,
    /// 2.0 in free space, about 3 in a furnished room, 3.8 and up in clutter.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub path_loss_exponent: f64,
    /// Loss the datasheet figures leave out: PCB antennas, enclosures, a hand
    /// around the tracker.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub implementation_loss_db: f64,
    /// Standard deviation of the slow fading in dB.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fading_db: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub walls: Vec<Wall>,
}

impl Default for Radio {
    fn default() -> Self {
        Self {
            tx_power_dbm: 4.0,
            sensitivity_dbm: -94.0,
            path_loss_exponent: 3.0,
            implementation_loss_db: 10.0,
            fading_db: 2.0,
            walls: Vec::new(),
        }
    }
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkQuality {
    /// Since: Odyssey Desktop 0.7.5.
    Strong,
    /// Since: Odyssey Desktop 0.7.5.
    Fair,
    /// Since: Odyssey Desktop 0.7.5.
    Marginal,
    /// Since: Odyssey Desktop 0.7.5.
    Lost,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// Since: Odyssey Desktop 0.7.5.
    pub distance_m: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub rssi_dbm: f64,
    /// Headroom over the receiver's sensitivity.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub margin_db: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub quality: LinkQuality,
    /// Expected fraction of packets lost at this margin.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub loss: f64,
}

impl Radio {
    /// # Errors
    ///
    /// Returns an error if radio settings or wall data are invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn validate(&self) -> Result<(), StageError> {
        let ok = [
            self.tx_power_dbm,
            self.sensitivity_dbm,
            self.path_loss_exponent,
            self.implementation_loss_db,
            self.fading_db,
        ]
        .iter()
        .all(|value| value.is_finite())
            && (1.0..=6.0).contains(&self.path_loss_exponent)
            && (0.0..=40.0).contains(&self.implementation_loss_db)
            && self.fading_db >= 0.0
            && self.walls.len() <= 64
            && self.walls.iter().all(|wall| {
                wall.a.iter().chain(&wall.b).all(|v| v.is_finite())
                    && wall.loss_db.is_finite()
                    && wall.loss_db >= 0.0
            });
        if ok {
            Ok(())
        } else {
            Err(StageError::invalid("invalid radio environment"))
        }
    }

    /// Received power for a transmitter at `from` and receiver at `to` (stage
    /// positions). `link` selects an independent fading sequence.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn rssi_dbm(&self, from: [f64; 3], to: [f64; 3], t_s: f64, link: u64) -> f64 {
        let distance = distance(from, to).max(MIN_DISTANCE_M);
        let walls: f64 = self
            .walls
            .iter()
            .filter(|wall| crosses([from[0], from[2]], [to[0], to[2]], wall.a, wall.b))
            .map(|wall| wall.loss_db)
            .sum();
        // Two incommensurate slow sines stand in for multipath fading, so a run
        // repeats exactly. Their rms is `FADE_RMS`, and dividing by it makes
        // `fading_db` the standard deviation.
        let phase = link as f64 * 1.618;
        let fade = self.fading_db
            * (0.7 * (0.9 * t_s + phase).sin() + 0.5 * (2.3 * t_s + 2.0 * phase).sin())
            / FADE_RMS;
        self.tx_power_dbm
            - (self.loss_at_1m_db() + 10.0 * self.path_loss_exponent * distance.log10() + walls)
            + fade
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn link(&self, from: [f64; 3], to: [f64; 3], t_s: f64, link: u64) -> Link {
        let rssi_dbm = self.rssi_dbm(from, to, t_s, link);
        let margin_db = rssi_dbm - self.sensitivity_dbm;
        Link {
            distance_m: distance(from, to),
            rssi_dbm,
            margin_db,
            quality: match margin_db {
                m if m >= 20.0 => LinkQuality::Strong,
                m if m >= 10.0 => LinkQuality::Fair,
                m if m >= 0.0 => LinkQuality::Marginal,
                _ => LinkQuality::Lost,
            },
            loss: 1.0 / (1.0 + ((margin_db + 1.2) / 1.5).exp()),
        }
    }

    fn loss_at_1m_db(&self) -> f64 {
        LOSS_AT_1M_DB + self.implementation_loss_db
    }

    /// Distance at which the margin reaches zero without walls or fading.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn range_m(&self) -> f64 {
        let budget = self.tx_power_dbm - self.sensitivity_dbm - self.loss_at_1m_db();
        10.0_f64.powf(budget / (10.0 * self.path_loss_exponent))
    }

    /// Signal strength against distance in open air, for plotting.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn curve(&self, max_m: f64, points: usize) -> Vec<[f64; 2]> {
        (0..points)
            .map(|i| {
                let d = MIN_DISTANCE_M
                    + (max_m - MIN_DISTANCE_M) * i as f64 / (points - 1).max(1) as f64;
                [
                    d,
                    self.tx_power_dbm
                        - (self.loss_at_1m_db() + 10.0 * self.path_loss_exponent * d.log10()),
                ]
            })
            .collect()
    }
}

pub(crate) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether segments `p1..p2` and `q1..q2` cross.
fn crosses(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    let side = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let (d1, d2) = (side(q1, q2, p1), side(q1, q2, p2));
    let (d3, d4) = (side(p1, p2, q1), side(p1, p2, q2));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Radio {
        Radio {
            fading_db: 0.0,
            ..Radio::default()
        }
    }

    #[test]
    fn signal_falls_six_db_per_doubling_in_free_space() {
        let radio = Radio {
            path_loss_exponent: 2.0,
            ..quiet()
        };
        let origin = [0.0; 3];
        let near = radio.rssi_dbm(origin, [0.0, 0.0, 2.0], 0.0, 0);
        let far = radio.rssi_dbm(origin, [0.0, 0.0, 4.0], 0.0, 0);
        assert!((near - far - 6.02).abs() < 0.01, "{}", near - far);
    }

    #[test]
    fn range_is_where_the_margin_reaches_zero() {
        let radio = quiet();
        let link = radio.link([0.0; 3], [0.0, 0.0, radio.range_m()], 0.0, 0);
        assert!(link.margin_db.abs() < 0.01, "{link:?}");
        assert!((link.loss - 0.308).abs() < 0.01, "{link:?}");
    }

    #[test]
    fn a_wall_between_the_radios_costs_its_loss_only_when_crossed() {
        let radio = Radio {
            walls: vec![Wall {
                a: [1.0, -5.0],
                b: [1.0, 5.0],
                loss_db: 9.0,
            }],
            ..quiet()
        };
        let open = Radio {
            walls: Vec::new(),
            ..radio.clone()
        };
        let (a, b) = ([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]);
        let drop = open.rssi_dbm(a, b, 0.0, 0) - radio.rssi_dbm(a, b, 0.0, 0);
        assert!((drop - 9.0).abs() < 1e-9);
        let same_side = [0.5, 0.0, 1.0];
        assert_eq!(
            open.rssi_dbm(a, same_side, 0.0, 0),
            radio.rssi_dbm(a, same_side, 0.0, 0)
        );
    }

    #[test]
    fn quality_follows_the_margin() {
        let radio = quiet();
        let at = |d: f64| radio.link([0.0; 3], [0.0, 0.0, d], 0.0, 0).quality;
        assert_eq!(at(1.0), LinkQuality::Strong);
        assert_eq!(at(radio.range_m() * 0.9), LinkQuality::Marginal);
        assert_eq!(at(radio.range_m() * 1.5), LinkQuality::Lost);
    }

    #[test]
    fn fading_stays_within_a_few_sigma() {
        let radio = Radio::default();
        let (a, b) = ([0.0; 3], [0.0, 0.0, 3.0]);
        let steady = quiet().rssi_dbm(a, b, 0.0, 3);
        for step in 0..1000 {
            let delta = radio.rssi_dbm(a, b, f64::from(step) * 0.1, 3) - steady;
            assert!(delta.abs() < 3.0 * radio.fading_db + 1e-9, "{delta}");
        }
    }
}
