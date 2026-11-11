use nalgebra::{Matrix3, Point2, Point3, UnitQuaternion};
use odyssey_common::ScreenCalibration;
use odyssey_protocol::{GeneralConfig, MarkersReport};

use super::SimError;
use crate::dataset::{ScenePose, SceneScreen, project_world};

const WIDTH: f32 = 1.2;
const HEIGHT: f32 = 0.675;
// Established tracker layout order, shared with the autocalibration synthetic scene.
const MARKERS: [[f32; 2]; 6] = [
    [0.2, 1.0],
    [0.2, 0.0],
    [0.8, 0.0],
    [0.5, 0.0],
    [0.5, 1.0],
    [0.8, 1.0],
];
/// Marker coordinates the simulated sensor reports.
const IMAGE_SIZE_PX: [u32; 2] = [640, 480];
const AIM_DISTANCE_M: f64 = 2.0;
const MAX_SCREENS: usize = odyssey_common::MAX_SCREEN_ID as usize + 1;

/// Screen ID 0 of a simulator nobody has given screens. No user calibration files
/// should be overwritten. `Aim` views its physical XY plane from two meters away.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn synthetic_screen_calibration() -> ScreenCalibration<f32> {
    ScreenCalibration {
        surface: None,
        rotation: UnitQuaternion::identity(),
        homography: Matrix3::new(1.0 / WIDTH, 0.0, 0.0, 0.0, 1.0 / HEIGHT, 0.0, 0.0, 0.0, 1.0),
        object_points: MARKERS.map(|[x, y]| Point3::new(x * WIDTH, y * HEIGHT, 0.0)),
    }
}

/// The displays the units look at.
pub(crate) struct World {
    screens: Vec<SceneScreen>,
    /// Every screen's markers in world meters, kept so a report does not
    /// revalidate the screens.
    markers: Vec<[f64; 3]>,
}

impl World {
    pub(crate) fn new(screens: Vec<SceneScreen>) -> Result<Self, SimError> {
        if screens.len() > MAX_SCREENS {
            return Err(SimError::InvalidInput("too many screens"));
        }
        let mut markers = Vec::with_capacity(screens.len() * MARKERS.len());
        for (index, screen) in screens.iter().enumerate() {
            if screens[..index].iter().any(|other| other.id == screen.id) {
                return Err(SimError::InvalidInput("duplicate screen ID"));
            }
            let geometry = screen
                .geometry()
                .map_err(|_error| SimError::InvalidInput("invalid screen calibration"))?;
            markers.extend((0..MARKERS.len()).filter_map(|marker| geometry.marker_world(marker)));
        }
        Ok(Self { screens, markers })
    }

    pub(crate) fn synthetic() -> Self {
        Self::new(vec![SceneScreen::from_calibration(
            0,
            synthetic_screen_calibration(),
        )])
        .expect("the synthetic screen is a valid calibration")
    }

    /// The camera pose squarely in front of `uv` on the first screen.
    pub(crate) fn aim_pose(&self, uv: [f32; 2]) -> Option<ScenePose> {
        let screen = self.screens.first()?;
        let geometry = screen.geometry().ok()?;
        let target = geometry
            .point_at_uv([f64::from(uv[0]), f64::from(uv[1])])
            .ok()?;
        let back = geometry.screen_to_world([0.0, 0.0, -AIM_DISTANCE_M]);
        let facing = screen.calibration.rotation.inverse();
        Some(ScenePose {
            position_m: std::array::from_fn(|axis| target[axis] + back[axis]),
            orientation_xyzw: [facing.i, facing.j, facing.k, facing.w].map(f64::from),
        })
    }

    /// What the unit's near-field sensor reports from `pose`: the visible
    /// markers, in no particular order, as the sensor reports its blobs.
    pub(crate) fn markers(&self, pose: &ScenePose, config: &GeneralConfig) -> MarkersReport {
        let mut points = [Point2::origin(); 16];
        let GeneralConfig::CameraModelNf(camera) = config else {
            return MarkersReport { points };
        };
        // The stored matrix is column-major.
        let k = camera.k.map(f64::from);
        let intrinsics = [[k[0], k[3], k[6]], [0.0, k[4], k[7]], [0.0, 0.0, 1.0]];
        let distortion = camera.distortion.map(f64::from);
        let visible = self.markers.iter().filter_map(|marker| {
            let [u, v] = project_world(IMAGE_SIZE_PX, &intrinsics, &distortion, pose, *marker)?;
            ((1.0..=f64::from(IMAGE_SIZE_PX[0] - 1)).contains(&u)
                && (1.0..=f64::from(IMAGE_SIZE_PX[1] - 1)).contains(&v))
            .then(|| Point2::new(u.round() as u16, v.round() as u16))
        });
        for (slot, point) in points.iter_mut().zip(visible) {
            *slot = point;
        }
        MarkersReport { points }
    }
}

#[cfg(test)]
mod tests {
    use odyssey_protocol::wire::CameraCalibrationParams;

    use super::*;

    fn camera() -> GeneralConfig {
        GeneralConfig::CameraModelNf(
            CameraCalibrationParams {
                camera_matrix: [500.0, 0.0, 320.0, 0.0, 500.0, 240.0, 0.0, 0.0, 1.0],
                dist_coeffs: [0.0; 5],
            }
            .into(),
        )
    }

    #[test]
    fn aiming_at_a_point_centers_it_in_the_image() {
        let world = World::synthetic();
        let pose = world.aim_pose([0.5, 0.0]).unwrap();
        // Marker 3 sits at the top center of the screen, where the camera aims.
        let report = world.markers(&pose, &camera());
        assert!(report.points.contains(&Point2::new(320, 240)));
        assert_eq!(
            report
                .points
                .iter()
                .filter(|p| **p != Point2::origin())
                .count(),
            6
        );
    }

    #[test]
    fn a_rotated_screen_is_aimed_at_along_its_own_normal() {
        let mut calibration = synthetic_screen_calibration();
        calibration.rotation = UnitQuaternion::from_euler_angles(0.2, -0.4, 0.1);
        let rotated = World::new(vec![SceneScreen::from_calibration(0, calibration)]).unwrap();
        let flat = World::synthetic();
        let pose = rotated.aim_pose([0.3, 0.55]).unwrap();
        assert_eq!(
            rotated.markers(&pose, &camera()).points,
            flat.markers(&flat.aim_pose([0.3, 0.55]).unwrap(), &camera())
                .points
        );
    }

    #[test]
    fn invalid_worlds_are_rejected() {
        let screen = SceneScreen::from_calibration(0, synthetic_screen_calibration());
        assert!(World::new(vec![screen.clone(), screen]).is_err());
        let mut degenerate = synthetic_screen_calibration();
        degenerate.object_points = [Point3::origin(); 6];
        assert!(World::new(vec![SceneScreen::from_calibration(0, degenerate)]).is_err());
        assert!(
            World::new(Vec::new())
                .unwrap()
                .aim_pose([0.5, 0.5])
                .is_none()
        );
    }
}
