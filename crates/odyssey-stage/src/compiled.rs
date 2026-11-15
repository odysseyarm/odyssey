use nalgebra::{Quaternion, UnitQuaternion, Vector3};
use odyssey_sim::dataset::{SceneDescription, SceneScreen};
use odyssey_sim::surface::ScreenGeometry;
use serde::Serialize;

use crate::StageError;
use crate::pattern::trigger_times;
use crate::radio::{Link, distance};
use crate::stage::{Device, DeviceKind, Stage, Tracker};

/// Largest hand offset that is retried at half size until the ray lands on the
/// display. Handling never aims a tracker off the screen.
const OFFSET_HALVINGS: u32 = 8;

/// A stage with its display validated and built once.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug)]
pub struct Compiled {
    stage: Stage,
    screen: SceneScreen,
}

/// A tracker's pose at one instant.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pose {
    /// Since: Odyssey Desktop 0.7.5.
    pub position_m: [f64; 3],
    /// Camera-to-world rotation as x, y, z, w.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub orientation_xyzw: [f64; 4],
    /// Where the tracker actually points, in display fractions.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub aim_uv: [f64; 2],
    /// Since: Odyssey Desktop 0.7.5.
    pub distance_m: f64,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, Serialize)]
pub struct LinkReport {
    /// Since: Odyssey Desktop 0.7.5.
    pub dongle: String,
    /// Since: Odyssey Desktop 0.7.5.
    pub endpoint: String,
    /// Since: Odyssey Desktop 0.7.5.
    pub best: bool,
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(flatten)]
    pub link: Link,
}

impl Stage {
    /// Compile stage into validated runtime data.
    ///
    /// # Errors
    ///
    /// Returns an error if stage configuration or display calibration is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn compile(&self) -> Result<Compiled, StageError> {
        self.validate()?;
        Ok(Compiled {
            screen: self.display.screen()?,
            stage: self.clone(),
        })
    }
}

impl Compiled {
    /// Since: Odyssey Desktop 0.7.5.
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn screen(&self) -> &SceneScreen {
        &self.screen
    }

    /// # Errors
    ///
    /// Returns an error if screen calibration has invalid geometry.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn geometry(&self) -> Result<ScreenGeometry<'_>, StageError> {
        Ok(self.screen.geometry()?)
    }

    /// Marker positions in stage coordinates.
    ///
    /// # Errors
    ///
    /// Returns an error if screen has no usable marker positions.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn markers(&self) -> Result<[[f64; 3]; 6], StageError> {
        let geometry = self.geometry()?;
        let mut markers = [[0.0; 3]; 6];
        for (index, marker) in markers.iter_mut().enumerate() {
            *marker = geometry
                .marker_world(index)
                .ok_or_else(|| StageError::invalid("display has no markers"))?;
        }
        Ok(markers)
    }

    /// The pose of a tracker at `t_s`, loop-free: callers wrap time themselves.
    ///
    /// # Errors
    ///
    /// Returns an error if device is not a tracker.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn pose(
        &self,
        geometry: &ScreenGeometry<'_>,
        device: &Device,
        t_s: f64,
    ) -> Result<Pose, StageError> {
        let DeviceKind::Tracker(tracker) = &device.kind else {
            return Err(StageError::NotATracker(device.id.clone()));
        };
        tracker_pose(geometry, device, tracker, t_s)
    }

    /// Current position of any device, following a tracker's movement.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn position(&self, device: &Device, t_s: f64) -> [f64; 3] {
        match &device.kind {
            DeviceKind::Tracker(tracker) => tracker.movement.position(device.position_m, t_s),
            DeviceKind::Dongle => device.position_m,
        }
    }

    /// Every dongle-to-endpoint link at `t_s`. Endpoints are the trackers and
    /// the six display markers. The strongest dongle per endpoint is `best`.
    ///
    /// # Errors
    ///
    /// Returns an error if display markers cannot be computed.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn links(&self, t_s: f64) -> Result<Vec<LinkReport>, StageError> {
        self.links_with(t_s, |_| None)
    }

    /// As [`Self::links`], with `measured` supplying the position of devices
    /// whose location comes from hardware instead of the staged movement.
    ///
    /// # Errors
    ///
    /// Returns an error if display markers cannot be computed.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn links_with(
        &self,
        t_s: f64,
        measured: impl Fn(&Device) -> Option<[f64; 3]>,
    ) -> Result<Vec<LinkReport>, StageError> {
        let markers = self.markers()?;
        let mut endpoints: Vec<(String, [f64; 3])> = self
            .stage
            .devices
            .iter()
            .filter(|device| matches!(device.kind, DeviceKind::Tracker(_)))
            .map(|device| {
                let position = measured(device).unwrap_or_else(|| self.position(device, t_s));
                (device.id.clone(), position)
            })
            .collect();
        endpoints.extend(
            markers
                .iter()
                .enumerate()
                .map(|(index, position)| (format!("marker-{index}"), *position)),
        );
        let dongles: Vec<&Device> = self
            .stage
            .devices
            .iter()
            .filter(|device| matches!(device.kind, DeviceKind::Dongle))
            .collect();
        let mut reports = Vec::with_capacity(dongles.len() * endpoints.len());
        for (endpoint_index, (endpoint, position)) in endpoints.iter().enumerate() {
            let first = reports.len();
            for (dongle_index, dongle) in dongles.iter().enumerate() {
                let key = (dongle_index * 1000 + endpoint_index) as u64;
                reports.push(LinkReport {
                    dongle: dongle.id.clone(),
                    endpoint: endpoint.clone(),
                    best: false,
                    link: self
                        .stage
                        .radio
                        .link(dongle.position_m, *position, t_s, key),
                });
            }
            if let Some(best) = reports[first..]
                .iter_mut()
                .max_by(|a, b| a.link.rssi_dbm.total_cmp(&b.link.rssi_dbm))
            {
                best.best = true;
            }
        }
        Ok(reports)
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn trigger_times(&self, device: &Device) -> Vec<f64> {
        device
            .tracker()
            .map_or_else(Vec::new, |tracker| trigger_times(&tracker.presses))
    }

    pub(crate) fn description(&self) -> SceneDescription {
        crate::scene::empty(&self.screen)
    }
}

fn tracker_pose(
    geometry: &ScreenGeometry<'_>,
    device: &Device,
    tracker: &Tracker,
    t_s: f64,
) -> Result<Pose, StageError> {
    let position = tracker.movement.position(device.position_m, t_s);
    let target = geometry.point_at_uv(tracker.pattern.uv(t_s))?;
    let forward = Vector3::from(target) - Vector3::from(position);
    if forward.norm() < 1e-6 {
        return Err(StageError::invalid(format!(
            "tracker {:?} sits on the display",
            device.id
        )));
    }
    let facing = UnitQuaternion::face_towards(&forward, &Vector3::y());
    let roll = UnitQuaternion::from_axis_angle(&Vector3::z_axis(), tracker.roll_deg.to_radians());
    let (yaw, pitch) = tracker
        .handling
        .angles(t_s, &trigger_times(&tracker.presses));
    for halving in 0..=OFFSET_HALVINGS {
        let scale = if halving == OFFSET_HALVINGS {
            0.0
        } else {
            0.5_f64.powi(halving as i32)
        };
        let shake = UnitQuaternion::from_axis_angle(&Vector3::y_axis(), yaw * scale)
            * UnitQuaternion::from_axis_angle(&Vector3::x_axis(), pitch * scale);
        let orientation = facing * roll * shake;
        let direction: [f64; 3] = orientation.transform_vector(&Vector3::z()).into();
        if let Some(hit) = geometry.raycast(position, direction)? {
            let q: &Quaternion<f64> = orientation.quaternion();
            return Ok(Pose {
                position_m: position,
                orientation_xyzw: [q.i, q.j, q.k, q.w],
                aim_uv: hit.uv,
                distance_m: distance(position, hit.position_world_m),
            });
        }
    }
    Err(StageError::invalid(format!(
        "tracker {:?} cannot see the display",
        device.id
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Display, Waviness};
    use crate::pattern::Pattern;
    use crate::stage::Binding;

    fn stage_with(display: Display, pattern: Pattern) -> Compiled {
        let mut stage = Stage {
            display,
            ..Stage::default()
        };
        if let DeviceKind::Tracker(tracker) = &mut stage.devices[0].kind {
            tracker.pattern = pattern;
        }
        stage.compile().unwrap()
    }

    #[test]
    fn a_held_aim_lands_on_its_display_point_even_on_a_wavy_screen() {
        let display = Display {
            waviness: Waviness {
                amplitude_m: 0.05,
                ..Waviness::default()
            },
            ..Display::default()
        };
        let compiled = stage_with(display, Pattern::Hold { at: [0.3, 0.7] });
        let geometry = compiled.geometry().unwrap();
        let pose = compiled
            .pose(&geometry, &compiled.stage().devices[0], 0.0)
            .unwrap();
        assert!((pose.aim_uv[0] - 0.3).abs() < 1e-6 && (pose.aim_uv[1] - 0.7).abs() < 1e-6);
        assert!(pose.distance_m > 1.5);
    }

    #[test]
    fn handling_cannot_push_the_aim_off_the_display() {
        let mut compiled = stage_with(Display::default(), Pattern::Hold { at: [0.02, 0.02] });
        let mut stage = compiled.stage().clone();
        if let DeviceKind::Tracker(tracker) = &mut stage.devices[0].kind {
            tracker.handling.drift_deg = 20.0;
            tracker.handling.tremor_deg = 10.0;
        }
        compiled = stage.compile().unwrap();
        let geometry = compiled.geometry().unwrap();
        for step in 0..200 {
            compiled
                .pose(
                    &geometry,
                    &compiled.stage().devices[0],
                    f64::from(step) * 0.1,
                )
                .unwrap();
        }
    }

    #[test]
    fn links_name_the_best_dongle_per_endpoint() {
        let mut stage = Stage::default();
        stage.devices.push(Device {
            id: "dongle-2".to_owned(),
            name: "Far dongle".to_owned(),
            position_m: [0.6, 0.3, -9.0],
            binding: Binding::Stage,
            kind: DeviceKind::Dongle,
        });
        let links = stage.compile().unwrap().links(0.0).unwrap();
        assert_eq!(links.len(), 2 * 7);
        let best: Vec<_> = links.iter().filter(|link| link.best).collect();
        assert_eq!(best.len(), 7);
        assert!(best.iter().all(|link| link.dongle == "dongle-1"));
    }
}
