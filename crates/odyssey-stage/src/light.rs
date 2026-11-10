//! Infrared background that competes with the display markers.
//!
//! A qualitative model: a marker is detected when its signal, falling with the
//! square of distance, clears a share of the background around it. Real sensor
//! saturation and exposure behavior need measured data before numbers here can
//! support an accuracy claim.

use nalgebra::Vector3;
use serde::{Deserialize, Serialize};

use crate::StageError;

/// A marker at 2 m has signal 1.0.
const REFERENCE_DISTANCE_M: f64 = 2.0;
/// Angular radius of the glare patch around a light source, in degrees.
const GLARE_RADIUS_DEG: f64 = 8.0;
/// Signal must reach this share of the background.
const DETECTION_RATIO: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightSource {
    pub position_m: [f64; 3],
    /// Background added around the source's image on the sensor. 1.0 matches a
    /// marker seen from 2 m; a sunlit window is far above that.
    pub intensity: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    /// Infrared everywhere in the scene: 0 is a dark room, 1 is bright daylight.
    pub ambient: f64,
    pub sources: Vec<LightSource>,
}

impl Lighting {
    pub fn validate(&self) -> Result<(), StageError> {
        let ok = self.ambient.is_finite()
            && (0.0..=10.0).contains(&self.ambient)
            && self.sources.len() <= 16
            && self.sources.iter().all(|source| {
                source.position_m.iter().all(|v| v.is_finite())
                    && source.intensity.is_finite()
                    && source.intensity >= 0.0
            });
        if ok {
            Ok(())
        } else {
            Err(StageError::invalid("invalid lighting"))
        }
    }

    /// Whether a tracker at `camera` looking along `forward` can pick out a
    /// marker at `marker`.
    pub fn marker_detected(&self, camera: [f64; 3], forward: [f64; 3], marker: [f64; 3]) -> bool {
        let to_marker = Vector3::from(marker) - Vector3::from(camera);
        let distance = to_marker.norm();
        if distance <= 0.0 {
            return false;
        }
        let signal = (REFERENCE_DISTANCE_M / distance).powi(2);
        let mut background = self.ambient;
        let glare = GLARE_RADIUS_DEG.to_radians();
        for source in &self.sources {
            let to_source = Vector3::from(source.position_m) - Vector3::from(camera);
            if to_source.dot(&Vector3::from(forward)) <= 0.0 {
                continue;
            }
            let angle = to_source.angle(&to_marker);
            background += source.intensity * (-(angle / glare).powi(2)).exp();
        }
        signal >= DETECTION_RATIO * background
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMERA: [f64; 3] = [0.0, 0.0, -2.0];
    const FORWARD: [f64; 3] = [0.0, 0.0, 1.0];

    #[test]
    fn dark_room_sees_distant_markers_and_daylight_does_not() {
        let dark = Lighting::default();
        let day = Lighting {
            ambient: 1.0,
            ..Lighting::default()
        };
        let near = [0.0, 0.0, 0.0];
        let far = [0.0, 0.0, 6.0];
        assert!(dark.marker_detected(CAMERA, FORWARD, far));
        assert!(day.marker_detected(CAMERA, FORWARD, near));
        assert!(!day.marker_detected(CAMERA, FORWARD, far));
    }

    #[test]
    fn a_lamp_next_to_a_marker_hides_it_but_not_a_distant_one() {
        let lamp = Lighting {
            ambient: 0.0,
            sources: vec![LightSource {
                position_m: [0.05, 0.0, 0.0],
                intensity: 20.0,
            }],
        };
        assert!(!lamp.marker_detected(CAMERA, FORWARD, [0.0, 0.0, 0.0]));
        assert!(lamp.marker_detected(CAMERA, FORWARD, [1.5, 0.0, 0.0]));
    }

    #[test]
    fn a_light_behind_the_tracker_causes_no_glare() {
        let lamp = Lighting {
            ambient: 0.0,
            sources: vec![LightSource {
                position_m: [0.0, 0.0, -5.0],
                intensity: 50.0,
            }],
        };
        assert!(lamp.marker_detected(CAMERA, FORWARD, [0.0, 0.0, 0.0]));
    }
}
