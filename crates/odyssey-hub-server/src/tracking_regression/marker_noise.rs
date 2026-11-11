//! Marker-report noise measured from a trace and applied to another.
//!
//! The camera module reports marker centroids, not images, so its blur,
//! saturation and threshold effects reach the tracker as centroid jitter and
//! missed detections. Measuring those two from a recording of the real sensor
//! and applying them to a synthetic trace gives the synthetic trace measured
//! noise instead of guessed noise. Units are whatever the reports use.

use serde::{Deserialize, Serialize};

use super::scenarios::Perturbation;
use super::{Error, Input, Trace};

/// Report positions farther than this from their neighbour in the adjacent
/// report are treated as different markers, not jitter.
const MATCH_GATE: f64 = 8.0;

/// Variance of rounding to whole report units, already present in any report.
const QUANTIZATION_VARIANCE: f64 = 1.0 / 12.0;

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerNoise {
    /// Per-axis centroid standard deviation, rounding to whole units included.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub centroid_sigma: f64,
    /// Chance a marker present in the reports either side of this one is missing.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub dropout_probability: f64,
}

/// A measured model with the amount of evidence behind it.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Measurement {
    /// Since: Odyssey Desktop 0.7.0.
    pub model: MarkerNoise,
    /// Per-axis second differences behind `centroid_sigma`.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub second_differences: usize,
    /// Markers seen in both neighbouring reports, behind `dropout_probability`.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub dropout_opportunities: usize,
}

impl MarkerNoise {
    fn validate(&self) -> Result<(), Error> {
        if !self.centroid_sigma.is_finite()
            || self.centroid_sigma < 0.0
            || !(0.0..=1.0).contains(&self.dropout_probability)
        {
            return Err(Error::Invalid(
                "marker noise needs a finite sigma >= 0 and a dropout probability in [0, 1]".into(),
            ));
        }
        Ok(())
    }
}

type MarkerReports = Vec<Vec<[f64; 2]>>;

/// One report channel per source and camera, each a sequence of detected points.
fn channels(trace: &Trace) -> Vec<MarkerReports> {
    let mut channels: Vec<(String, &'static str, MarkerReports)> = Vec::new();
    for sample in &trace.samples {
        let reports: &[(&'static str, &[[u16; 2]; 16])] = match &sample.input {
            Input::Markers { points } => &[("nf", points)],
            Input::Combined {
                nf_points,
                wf_points,
            } => &[("nf", nf_points), ("wf", wf_points)],
            Input::Imu { .. } | Input::Reset => continue,
        };
        for &(camera, points) in reports {
            let detected = points
                .iter()
                .filter(|p| **p != [0, 0])
                .map(|p| [f64::from(p[0]), f64::from(p[1])])
                .collect();
            match channels
                .iter_mut()
                .find(|(source, name, _)| *source == sample.source_id && *name == camera)
            {
                Some((_, _, frames)) => frames.push(detected),
                None => channels.push((sample.source_id.clone(), camera, vec![detected])),
            }
        }
    }
    channels.into_iter().map(|(_, _, frames)| frames).collect()
}

fn nearest(points: &[[f64; 2]], to: [f64; 2], gate: f64) -> Option<[f64; 2]> {
    points
        .iter()
        .map(|p| (p, (p[0] - to[0]).hypot(p[1] - to[1])))
        .filter(|(_, distance)| *distance <= gate)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| *p)
}

/// Estimate centroid jitter and dropout from consecutive reports.
///
/// Uses second differences `p[t-1] - 2 p[t] + p[t+1]`: smooth motion cancels
/// and white noise of variance s² leaves variance 6 s². Fast motion or
/// acceleration inflates the estimate, so measure a still or slowly moving
/// recording.
///
/// # Errors
/// Returns an error if no repeated detections provide both second-difference
/// samples and dropout opportunities.
///
/// # Since
/// Odyssey Desktop 0.7.0.
pub fn measure(trace: &Trace) -> Result<Measurement, Error> {
    let mut sum_squares = 0.0;
    let mut second_differences = 0;
    let mut dropped = 0;
    let mut dropout_opportunities = 0;
    for frames in channels(trace) {
        for window in frames.windows(3) {
            let [before, now, after] = [&window[0], &window[1], &window[2]];
            for &p in now {
                if let (Some(b), Some(a)) = (
                    nearest(before, p, MATCH_GATE),
                    nearest(after, p, MATCH_GATE),
                ) {
                    for axis in 0..2 {
                        let d2 = b[axis] - 2.0 * p[axis] + a[axis];
                        sum_squares += d2 * d2;
                        second_differences += 1;
                    }
                }
            }
            for &b in before {
                if nearest(after, b, 2.0 * MATCH_GATE).is_some() {
                    dropout_opportunities += 1;
                    if nearest(now, b, 2.0 * MATCH_GATE).is_none() {
                        dropped += 1;
                    }
                }
            }
        }
    }
    if second_differences == 0 || dropout_opportunities == 0 {
        return Err(Error::Invalid(
            "trace has too few repeated marker detections to measure noise".into(),
        ));
    }
    Ok(Measurement {
        model: MarkerNoise {
            centroid_sigma: (sum_squares / second_differences as f64 / 6.0).sqrt(),
            dropout_probability: f64::from(dropped) / dropout_opportunities as f64,
        },
        second_differences,
        dropout_opportunities,
    })
}

/// Gaussian sample with standard deviation 1 (Box-Muller).
#[expect(
    clippy::manual_midpoint,
    reason = "The generator is bounded to [-1, 1]; preserve seeded Gaussian samples"
)]
fn gaussian(random: &mut Perturbation) -> f64 {
    let u1 = ((random.next() + 1.0) / 2.0).max(f64::MIN_POSITIVE);
    let u2 = (random.next() + 1.0) / 2.0;
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Copy of `trace` whose marker reports carry `noise`, deterministic in `seed`.
///
/// Reports are already rounded to whole units, so only the variance beyond rounding is
/// added, and measuring the result gives back `centroid_sigma`. Truth, expectations
/// and IMU samples are unchanged. Provenance records the model.
///
/// # Errors
/// Returns an error if model parameters are invalid or applying them produces
/// a trace that fails validation.
///
/// # Since
/// Odyssey Desktop 0.7.0.
#[expect(
    clippy::manual_midpoint,
    reason = "The generator is bounded to [-1, 1]; preserve seeded dropout samples"
)]
pub fn apply(trace: &Trace, noise: MarkerNoise, seed: u64) -> Result<Trace, Error> {
    noise.validate()?;
    let added_sigma = (noise.centroid_sigma.powi(2) - QUANTIZATION_VARIANCE)
        .max(0.0)
        .sqrt();
    let mut random = Perturbation(seed);
    let mut noisy = trace.clone();
    for sample in &mut noisy.samples {
        let reports: Vec<&mut [[u16; 2]; 16]> = match &mut sample.input {
            Input::Markers { points } => vec![points],
            Input::Combined {
                nf_points,
                wf_points,
            } => vec![nf_points, wf_points],
            Input::Imu { .. } | Input::Reset => continue,
        };
        for points in reports {
            for point in points.iter_mut().filter(|p| **p != [0, 0]) {
                if (random.next() + 1.0) / 2.0 < noise.dropout_probability {
                    *point = [0, 0];
                    continue;
                }
                // Zero means "no marker", so a noisy centroid never lands on it.
                *point = point.map(|v| {
                    (f64::from(v) + added_sigma * gaussian(&mut random))
                        .round()
                        .clamp(1.0, f64::from(u16::MAX)) as u16
                });
            }
        }
    }
    noisy.provenance.description = format!(
        "{} Marker reports carry applied noise: centroid sigma {}, dropout probability {}, seed {seed}.",
        noisy.provenance.description, noise.centroid_sigma, noise.dropout_probability
    );
    noisy.validate()?;
    Ok(noisy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking_regression::scenarios::builtin_scenarios;

    fn stationary() -> Trace {
        builtin_scenarios()
            .into_iter()
            .find(|trace| trace.name == "stationary-center")
            .expect("stationary scenario")
    }

    #[test]
    fn applied_noise_measures_back_to_the_model() {
        let clean = stationary();
        assert!(measure(&clean).unwrap().model.centroid_sigma < 1e-9);
        for model in [
            MarkerNoise {
                centroid_sigma: 0.8,
                dropout_probability: 0.05,
            },
            MarkerNoise {
                centroid_sigma: 1.5,
                dropout_probability: 0.0,
            },
        ] {
            let measured = measure(&apply(&clean, model, 7).unwrap()).unwrap();
            let got = measured.model;
            assert!(
                (got.centroid_sigma / model.centroid_sigma - 1.0).abs() < 0.1,
                "{model:?} -> {measured:?}"
            );
            assert!(
                (got.dropout_probability - model.dropout_probability).abs() < 0.02,
                "{model:?} -> {measured:?}"
            );
            assert!(measured.second_differences > 1000, "{measured:?}");
        }
    }

    #[test]
    fn invalid_models_and_markerless_traces_are_rejected() {
        let clean = stationary();
        for model in [
            MarkerNoise {
                centroid_sigma: f64::NAN,
                dropout_probability: 0.0,
            },
            MarkerNoise {
                centroid_sigma: -1.0,
                dropout_probability: 0.0,
            },
            MarkerNoise {
                centroid_sigma: 0.0,
                dropout_probability: 1.5,
            },
        ] {
            assert!(apply(&clean, model, 1).is_err(), "{model:?}");
        }
        let mut imu_only = clean;
        imu_only
            .samples
            .retain(|sample| matches!(sample.input, Input::Imu { .. }));
        assert!(matches!(measure(&imu_only), Err(Error::Invalid(_))));
    }
}
