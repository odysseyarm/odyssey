//! Export of a staged run as the renderer-neutral scene Blender, Rerun and the
//! tracking regression runner already read.

use odyssey_sim::dataset::{
    SCENE_SCHEMA_VERSION, SceneCamera, SceneCoordinateBasis, SceneDescription, SceneFrame,
    ScenePose, SceneScreen, SceneTruth,
};

use crate::StageError;
use crate::compiled::Compiled;
use crate::stage::DeviceKind;

const MAX_FRAMES: usize = 20_000;
/// The tracker's near-field camera: 640×480 pinhole, 500 px focal length.
pub(crate) const IMAGE_SIZE_PX: [u32; 2] = [640, 480];
pub(crate) const FOCAL_PX: f64 = 500.0;

pub(crate) fn empty(screen: &SceneScreen) -> SceneDescription {
    SceneDescription {
        schema_version: SCENE_SCHEMA_VERSION,
        provenance: "Staged by odyssey-stage; geometry-only labels, instantaneous exposure, no physical capture.".to_owned(),
        coordinate_basis: SceneCoordinateBasis::ScreenXRightYDownZForward,
        screens: vec![screen.clone()],
        cameras: Vec::new(),
        frames: Vec::new(),
    }
}

impl Compiled {
    /// One tracker's run as scene frames at `fps`. Truth and marker visibility
    /// come from the shared relabel pass. Markers hidden by the stage lighting
    /// are then removed from the visible list.
    ///
    /// # Errors
    ///
    /// Returns an error if device, frame rate, or generated scene is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn scene(&self, device_id: &str, fps: f64) -> Result<SceneDescription, StageError> {
        let stage = self.stage();
        let device = stage.device(device_id)?;
        if !matches!(device.kind, DeviceKind::Tracker(_)) {
            return Err(StageError::NotATracker(device_id.to_owned()));
        }
        if !fps.is_finite() || !(1.0..=240.0).contains(&fps) {
            return Err(StageError::invalid("fps must be 1 to 240"));
        }
        let count = (stage.duration_s * fps).floor() as usize + 1;
        if count > MAX_FRAMES {
            return Err(StageError::invalid(format!(
                "{count} frames exceeds the limit of {MAX_FRAMES}; shorten the run or lower fps"
            )));
        }
        let geometry = self.geometry()?;
        let camera_id = format!("{device_id}-nf");
        let mut scene = self.description();
        scene.cameras.push(SceneCamera {
            id: camera_id.clone(),
            source_id: device_id.to_owned(),
            image_size_px: IMAGE_SIZE_PX,
            intrinsics: [
                [FOCAL_PX, 0.0, f64::from(IMAGE_SIZE_PX[0]) / 2.0],
                [0.0, FOCAL_PX, f64::from(IMAGE_SIZE_PX[1]) / 2.0],
                [0.0, 0.0, 1.0],
            ],
            distortion: [0.0; 5],
            sensor: None,
        });
        for index in 0..count {
            let t_s = index as f64 / fps;
            let pose = self.pose(&geometry, device, t_s)?;
            scene.frames.push(SceneFrame {
                id: format!("{device_id}-frame-{index:05}"),
                elapsed_us: (t_s * 1_000_000.0).round() as u64,
                exposure_us: 0,
                camera_id: camera_id.clone(),
                camera_to_world: ScenePose {
                    position_m: pose.position_m,
                    orientation_xyzw: pose.orientation_xyzw,
                },
                visible_marker_ids: Vec::new(),
                truth: SceneTruth {
                    aimpoint: pose.aim_uv,
                    position_m: None,
                    screen_id: 0,
                },
            });
        }
        scene.relabel()?;
        self.apply_lighting(&mut scene)?;
        Ok(scene)
    }

    fn apply_lighting(&self, scene: &mut SceneDescription) -> Result<(), StageError> {
        let lighting = &self.stage().lighting;
        if lighting.ambient == 0.0 && lighting.sources.is_empty() {
            return Ok(());
        }
        let markers = self.markers()?;
        let ids = &scene.screens[0].marker_ids;
        for frame in &mut scene.frames {
            let pose = &frame.camera_to_world;
            let [x, y, z, w] = pose.orientation_xyzw;
            let forward: [f64; 3] =
                nalgebra::UnitQuaternion::new_normalize(nalgebra::Quaternion::new(w, x, y, z))
                    .transform_vector(&nalgebra::Vector3::z())
                    .into();
            frame.visible_marker_ids.retain(|id| {
                ids.iter()
                    .position(|candidate| candidate == id)
                    .is_some_and(|index| {
                        lighting.marker_detected(pose.position_m, forward, markers[index])
                    })
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::light::Lighting;
    use crate::stage::Stage;

    #[test]
    fn scene_has_one_labelled_frame_per_tick_and_validates() {
        let stage = Stage {
            duration_s: 2.0,
            ..Stage::default()
        };
        let scene = stage.compile().unwrap().scene("tracker-1", 30.0).unwrap();
        assert_eq!(scene.frames.len(), 61);
        assert_eq!(scene.frames[30].elapsed_us, 1_000_000);
        scene.validate().unwrap();
        assert!(scene.frames.iter().all(|f| f.visible_marker_ids.len() == 6));
    }

    #[test]
    fn daylight_removes_distant_markers_from_the_labels() {
        let stage = Stage {
            duration_s: 1.0,
            lighting: Lighting {
                ambient: 6.0,
                sources: Vec::new(),
            },
            ..Stage::default()
        };
        let scene = stage.compile().unwrap().scene("tracker-1", 10.0).unwrap();
        assert!(scene.frames.iter().all(|f| f.visible_marker_ids.len() < 6));
    }

    #[test]
    fn dongles_and_oversized_runs_are_refused() {
        let stage = Stage {
            duration_s: 600.0,
            ..Stage::default()
        };
        let compiled = stage.compile().unwrap();
        compiled
            .scene("dongle-1", 30.0)
            .expect_err("a dongle has no scene");
        compiled
            .scene("tracker-1", 240.0)
            .expect_err("a run beyond the limit is refused");
        compiled
            .scene("nope", 30.0)
            .expect_err("an unknown device has no scene");
    }

    #[test]
    fn a_scene_display_survives_an_editor_with_another_origin() {
        use nalgebra::Matrix3;

        use crate::display::Display;
        use crate::stage::display_preset;

        let original = display_preset("tarp-on-a-wall").unwrap();
        let stage = Stage {
            display: original.clone(),
            duration_s: 0.5,
            ..Stage::default()
        };
        let compiled = stage.compile().unwrap();
        let mut scene = compiled.scene("tracker-1", 10.0).unwrap();
        let (dx, dy) = (0.3_f32, -0.2_f32);
        let calibration = &mut scene.screens[0].calibration;
        for point in &mut calibration.object_points {
            point.x += dx;
            point.y += dy;
        }
        for position in &mut calibration.surface.as_mut().unwrap().positions_m {
            position[0] += f64::from(dx);
            position[1] += f64::from(dy);
        }
        calibration.homography *= Matrix3::new(1.0, 0.0, -dx, 0.0, 1.0, -dy, 0.0, 0.0, 1.0);
        let adopted = Display::from_scene(&scene).unwrap();
        // The adopted origin is the planar fit's top-left corner, wherever the editor put it.
        let [top_left, top_right, ..] = compiled.screen().calibration.bounds().unwrap();
        assert!((f64::from((top_right - top_left).norm()) - adopted.width_m).abs() < 1e-3);
        let before = compiled.screen().calibration.surface.as_ref().unwrap();
        let imported = adopted.imported.as_ref().unwrap();
        let after = imported.calibration.surface.as_ref().unwrap();
        for (a, b) in before.positions_m.iter().zip(&after.positions_m) {
            let moved = [a[0] - f64::from(top_left.x), a[1] - f64::from(top_left.y)];
            assert!((moved[0] - b[0]).abs() < 1e-3 && (moved[1] - b[1]).abs() < 1e-3);
            assert!((a[2] - b[2]).abs() < 1e-6);
        }
        adopted.screen().unwrap();
    }
}
