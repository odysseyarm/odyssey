//! Display authoring: a few physical knobs become the calibrated surface the
//! tracker, simulator and Blender tools already share.

use nalgebra::{Matrix3, Point3, Quaternion, SMatrix, SVector, UnitQuaternion, Vector3};
use odyssey_common::{ScreenCalibration, ScreenSurface};
use odyssey_sim::dataset::{SceneDescription, SceneScreen};
use serde::{Deserialize, Serialize};

use crate::StageError;

/// Canonical marker layout as display fractions, in `object_points` order. Shared
/// with `odyssey_sim::synthetic_screen_calibration`.
///
/// Since: Odyssey Desktop 0.7.5.
pub const MARKER_UV: [[f64; 2]; 6] = [
    [0.2, 1.0],
    [0.2, 0.0],
    [0.8, 0.0],
    [0.5, 0.0],
    [0.5, 1.0],
    [0.8, 1.0],
];

const GRID_COLUMNS: u32 = 41;
const GRID_ROWS: u32 = 25;
/// Corners that anchor the planar fit: the four outer markers.
const FIT_MARKERS: [usize; 4] = [0, 1, 2, 5];
const MAX_KEYSTONE: f64 = 0.45;
const MAX_SIZE_M: f64 = 30.0;
const MIN_SIZE_M: f64 = 0.1;

/// Ripples in the screen, such as a pull-down projector screen or a loose wall
/// covering.
///
/// The screen stays flat along its top and bottom edges, where the markers sit:
/// the tracker treats the marker pattern as a plane.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Waviness {
    /// Peak displacement in meters.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub amplitude_m: f64,
    /// Number of ripple periods across the width.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub waves_u: f64,
    /// Ripple bumps down the height, rounded to a whole number of at least one.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub waves_v: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub phase_deg: f64,
}

impl Default for Waviness {
    fn default() -> Self {
        Self {
            amplitude_m: 0.0,
            waves_u: 1.5,
            waves_v: 1.0,
            phase_deg: 0.0,
        }
    }
}

/// Projector placement errors. Positive horizontal makes the top edge wider than
/// the bottom. Positive vertical makes the left edge taller than the right.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keystone {
    /// Since: Odyssey Desktop 0.7.5.
    pub horizontal: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub vertical: f64,
}

/// A display adopted from elsewhere: a scene file, such as one edited in
/// Blender, or the calibration of a running hub.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Imported {
    /// The display in stage coordinates: upright, its top-left at the origin.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub calibration: ScreenCalibration<f32>,
    /// Rotation from the source's world frame into the display's own, as x, y, z, w.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub rotation_xyzw: [f64; 4],
    /// The display's top-left corner in the source's display frame, in meters.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub origin_m: [f64; 2],
    /// The display is this screen of a running hub. The hub's simulated units
    /// then look at the hub's own copy of it.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_screen: Option<u8>,
}

impl Imported {
    /// A stage pose in the world frame the display was adopted from.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn to_source(
        &self,
        position_m: [f64; 3],
        orientation_xyzw: [f64; 4],
    ) -> ([f64; 3], [f64; 4]) {
        let [x, y, z, w] = self.rotation_xyzw;
        let to_world = UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z)).inverse();
        let [x, y, z, w] = orientation_xyzw;
        let orientation = to_world * UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z));
        let position = to_world
            * Vector3::new(
                position_m[0] + self.origin_m[0],
                position_m[1] + self.origin_m[1],
                position_m[2],
            );
        (
            position.into(),
            [orientation.i, orientation.j, orientation.k, orientation.w],
        )
    }
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    /// Since: Odyssey Desktop 0.7.5.
    pub width_m: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub height_m: f64,
    /// Since: Odyssey Desktop 0.7.5.
    pub waviness: Waviness,
    /// Since: Odyssey Desktop 0.7.5.
    pub keystone: Keystone,
    /// Lens distortion of the projector: positive bulges the picture outward at
    /// the edges (barrel), negative pinches it (pincushion).
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub barrel: f64,
    /// While set it replaces the generated surface, and the fields above only
    /// record its size.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<Imported>,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            width_m: 1.2,
            height_m: 0.675,
            waviness: Waviness::default(),
            keystone: Keystone::default(),
            barrel: 0.0,
            imported: None,
        }
    }
}

impl Display {
    /// A display with no distortion has no measured surface at all, matching a
    /// freshly calibrated planar screen.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn is_flat(&self) -> bool {
        self.waviness.amplitude_m == 0.0
            && self.keystone == Keystone::default()
            && self.barrel == 0.0
    }

    /// # Errors
    ///
    /// Returns an error if display dimensions or distortion parameters are invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn validate(&self) -> Result<(), StageError> {
        if self.imported.is_some() {
            return Ok(());
        }
        let finite = [
            self.width_m,
            self.height_m,
            self.waviness.amplitude_m,
            self.waviness.waves_u,
            self.waviness.waves_v,
            self.waviness.phase_deg,
            self.keystone.horizontal,
            self.keystone.vertical,
            self.barrel,
        ]
        .iter()
        .all(|value| value.is_finite());
        if !finite {
            return Err(StageError::invalid("display values must be finite"));
        }
        if !(MIN_SIZE_M..=MAX_SIZE_M).contains(&self.width_m)
            || !(MIN_SIZE_M..=MAX_SIZE_M).contains(&self.height_m)
        {
            return Err(StageError::invalid(format!(
                "display size must be {MIN_SIZE_M} to {MAX_SIZE_M} m"
            )));
        }
        if self.keystone.horizontal.abs() > MAX_KEYSTONE
            || self.keystone.vertical.abs() > MAX_KEYSTONE
        {
            return Err(StageError::invalid(format!(
                "keystone must be within ±{MAX_KEYSTONE}"
            )));
        }
        if !(-0.8..=0.8).contains(&self.barrel) {
            return Err(StageError::invalid("barrel must be within ±0.8"));
        }
        if self.waviness.waves_u < 0.0 || self.waviness.waves_v < 0.0 {
            return Err(StageError::invalid("wave counts cannot be negative"));
        }
        // A ripple needs about four grid cells per period to stay a ripple.
        if self.waviness.waves_u > f64::from(GRID_COLUMNS - 1) / 4.0
            || self.waviness.waves_v > f64::from(GRID_ROWS - 1) / 4.0
        {
            return Err(StageError::invalid("too many waves for the surface grid"));
        }
        Ok(())
    }

    /// Display-plane position in meters for a display fraction, origin at the
    /// top-left, +X right, +Y down, +Z away from the viewer.
    fn point(&self, u: f64, v: f64, corners: &[[f64; 2]; 4]) -> [f64; 3] {
        let (du, dv) = (u - 0.5, v - 0.5);
        let scale = 1.0 + self.barrel * (du * du + dv * dv);
        let [x, y] = apply(
            &corners_homography(corners),
            [0.5 + du * scale, 0.5 + dv * scale],
        );
        let wave = &self.waviness;
        let phase = wave.phase_deg.to_radians();
        let bumps = wave.waves_v.round().max(1.0);
        let ripple = wave.amplitude_m
            * (std::f64::consts::TAU * wave.waves_u * u + phase).sin()
            * (std::f64::consts::PI * bumps * v).sin();
        [x, y, ripple]
    }

    fn corners(&self) -> [[f64; 2]; 4] {
        let (w, h) = (self.width_m, self.height_m);
        let (kh, kv) = (self.keystone.horizontal, self.keystone.vertical);
        [
            [-kh * w / 2.0, -kv * h / 2.0],
            [w + kh * w / 2.0, kv * h / 2.0],
            [w - kh * w / 2.0, h - kv * h / 2.0],
            [kh * w / 2.0, h + kv * h / 2.0],
        ]
    }

    /// The calibrated screen the tracker and the tools consume.
    ///
    /// # Errors
    ///
    /// Returns an error if display calibration or geometry is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn screen(&self) -> Result<SceneScreen, StageError> {
        self.validate()?;
        let (w, h) = (self.width_m, self.height_m);
        let marker_ids = std::array::from_fn(|index| format!("screen-0-marker-{index}"));
        let calibration = if let Some(imported) = &self.imported {
            imported.calibration.clone()
        } else if self.is_flat() {
            ScreenCalibration {
                surface: None,
                rotation: UnitQuaternion::identity(),
                homography: Matrix3::new(
                    (1.0 / w) as f32,
                    0.0,
                    0.0,
                    0.0,
                    (1.0 / h) as f32,
                    0.0,
                    0.0,
                    0.0,
                    1.0,
                ),
                object_points: MARKER_UV
                    .map(|[u, v]| Point3::new((u * w) as f32, (v * h) as f32, 0.0)),
            }
        } else {
            self.measured_calibration()?
        };
        let screen = SceneScreen {
            id: 0,
            calibration,
            marker_ids,
        };
        screen.geometry()?;
        Ok(screen)
    }

    /// Adopt the single display of a scene file.
    ///
    /// # Errors
    ///
    /// Returns an error if scene does not contain exactly one usable screen.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn from_scene(scene: &SceneDescription) -> Result<Self, StageError> {
        let [screen] = scene.screens.as_slice() else {
            return Err(StageError::invalid(
                "the scene must have exactly one screen",
            ));
        };
        Self::from_screen(screen, false)
    }

    /// Adopt a calibrated screen. The origin moves to the display's top-left
    /// corner and the display stands upright, so devices keep their stage
    /// coordinates; `of_hub` marks it as a running hub's own screen.
    ///
    /// # Errors
    ///
    /// Returns an error if screen geometry or dimensions are invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn from_screen(screen: &SceneScreen, of_hub: bool) -> Result<Self, StageError> {
        screen.geometry()?;
        let mut calibration = screen.calibration.clone();
        let rotation = calibration.rotation.cast::<f64>();
        calibration.rotation = UnitQuaternion::identity();
        let [top_left, top_right, bottom_left, _] = calibration
            .bounds()
            .ok_or_else(|| StageError::invalid("the display has no usable bounds"))?;
        let (width_m, height_m) = (
            f64::from((top_right - top_left).norm()),
            f64::from((bottom_left - top_left).norm()),
        );
        if !(MIN_SIZE_M..=MAX_SIZE_M).contains(&width_m)
            || !(MIN_SIZE_M..=MAX_SIZE_M).contains(&height_m)
        {
            return Err(StageError::invalid(format!(
                "display size must be {MIN_SIZE_M} to {MAX_SIZE_M} m"
            )));
        }
        let shift = Vector3::new(top_left.x, top_left.y, 0.0);
        for point in &mut calibration.object_points {
            point.coords -= shift;
        }
        if let Some(surface) = &mut calibration.surface {
            for position in &mut surface.positions_m {
                position[0] -= f64::from(top_left.x);
                position[1] -= f64::from(top_left.y);
            }
        }
        calibration.homography *=
            Matrix3::new(1.0, 0.0, top_left.x, 0.0, 1.0, top_left.y, 0.0, 0.0, 1.0);
        Ok(Self {
            width_m,
            height_m,
            imported: Some(Imported {
                calibration,
                rotation_xyzw: [rotation.i, rotation.j, rotation.k, rotation.w],
                origin_m: [f64::from(top_left.x), f64::from(top_left.y)],
                hub_screen: of_hub.then_some(screen.id),
            }),
            ..Self::default()
        })
    }

    /// The pose a hub's simulator is given for a stage pose. A display adopted
    /// from the hub keeps the hub's frame there. Any other is the stage's own.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn simulator_pose(
        &self,
        position_m: [f64; 3],
        orientation_xyzw: [f64; 4],
    ) -> ([f64; 3], [f64; 4]) {
        match &self.imported {
            Some(imported) if imported.hub_screen.is_some() => {
                imported.to_source(position_m, orientation_xyzw)
            }
            _ => (position_m, orientation_xyzw),
        }
    }

    /// Whether the hub's simulated units should look at the hub's own screens
    /// instead of this display.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn is_hub_screen(&self) -> bool {
        self.imported
            .as_ref()
            .is_some_and(|imported| imported.hub_screen.is_some())
    }

    fn measured_calibration(&self) -> Result<ScreenCalibration<f32>, StageError> {
        let corners = self.corners();
        let mut positions_m = Vec::with_capacity((GRID_COLUMNS * GRID_ROWS) as usize);
        for row in 0..GRID_ROWS {
            for column in 0..GRID_COLUMNS {
                positions_m.push(self.point(
                    f64::from(column) / f64::from(GRID_COLUMNS - 1),
                    f64::from(row) / f64::from(GRID_ROWS - 1),
                    &corners,
                ));
            }
        }
        let surface = ScreenSurface {
            columns: GRID_COLUMNS,
            rows: GRID_ROWS,
            positions_m,
        };
        surface
            .validate()
            .map_err(|error| StageError::invalid(format!("display folds over itself: {error}")))?;
        let object_points = MARKER_UV.map(|[u, v]| {
            let point = surface
                .point_at_uv(nalgebra::Point2::new(u, v))
                .unwrap_or_else(Vector3::zeros);
            Point3::new(point.x as f32, point.y as f32, point.z as f32)
        });
        // The homography stays the planar fit of the outer markers, as for a
        // calibrated real screen. The surface carries the rest.
        let source = FIT_MARKERS.map(|index| {
            let point = object_points[index];
            [f64::from(point.x), f64::from(point.y)]
        });
        let target = FIT_MARKERS.map(|index| MARKER_UV[index]);
        let homography = fit_homography(&source, &target)
            .ok_or_else(|| StageError::invalid("display markers are degenerate"))?;
        Ok(ScreenCalibration {
            surface: Some(surface),
            rotation: UnitQuaternion::identity(),
            homography: homography.cast::<f32>(),
            object_points,
        })
    }
}

/// Projective map from the unit square onto the four corners (TL, TR, BR, BL).
fn corners_homography(corners: &[[f64; 2]; 4]) -> Matrix3<f64> {
    fit_homography(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], corners)
        .unwrap_or_else(Matrix3::identity)
}

fn apply(homography: &Matrix3<f64>, [x, y]: [f64; 2]) -> [f64; 2] {
    let p = homography * Vector3::new(x, y, 1.0);
    [p.x / p.z, p.y / p.z]
}

/// Exact projective map taking four points to four points. `None` when the
/// points are degenerate.
fn fit_homography(source: &[[f64; 2]; 4], target: &[[f64; 2]; 4]) -> Option<Matrix3<f64>> {
    let mut a = SMatrix::<f64, 8, 8>::zeros();
    let mut b = SVector::<f64, 8>::zeros();
    for (i, ([x, y], [u, v])) in source.iter().zip(target).enumerate() {
        a.set_row(
            2 * i,
            &nalgebra::RowSVector::<f64, 8>::from_row_slice(&[
                *x,
                *y,
                1.0,
                0.0,
                0.0,
                0.0,
                -u * x,
                -u * y,
            ]),
        );
        a.set_row(
            2 * i + 1,
            &nalgebra::RowSVector::<f64, 8>::from_row_slice(&[
                0.0,
                0.0,
                0.0,
                *x,
                *y,
                1.0,
                -v * x,
                -v * y,
            ]),
        );
        b[2 * i] = *u;
        b[2 * i + 1] = *v;
    }
    let h = a.lu().solve(&b)?;
    h.iter()
        .all(|value| value.is_finite())
        .then(|| Matrix3::new(h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wavy() -> Display {
        Display {
            waviness: Waviness {
                amplitude_m: 0.05,
                ..Waviness::default()
            },
            ..Display::default()
        }
    }

    #[test]
    fn flat_display_matches_the_synthetic_calibration() {
        let screen = Display::default().screen().unwrap();
        let reference = odyssey_sim::synthetic_screen_calibration();
        assert!(screen.calibration.surface.is_none());
        assert!(
            (screen.calibration.homography - reference.homography)
                .abs()
                .max()
                < 1e-6
        );
        for (ours, theirs) in screen
            .calibration
            .object_points
            .iter()
            .zip(&reference.object_points)
        {
            assert!((ours - theirs).norm() < 1e-6);
        }
    }

    #[test]
    fn waves_displace_the_surface_and_markers_follow_it() {
        let screen = wavy().screen().unwrap();
        let surface = screen.calibration.surface.as_ref().unwrap();
        let depth = surface
            .positions_m
            .iter()
            .map(|p| p[2].abs())
            .fold(0.0, f64::max);
        assert!(depth > 0.03 && depth <= 0.05 + 1e-9, "{depth}");
        let marker = screen.calibration.object_points[3];
        let on_surface = surface
            .point_at_uv(nalgebra::Point2::new(0.5, 0.0))
            .unwrap();
        assert!((f64::from(marker.z) - on_surface.z).abs() < 1e-6);
    }

    #[test]
    fn keystone_makes_the_top_edge_wider() {
        let display = Display {
            keystone: Keystone {
                horizontal: 0.2,
                vertical: 0.0,
            },
            ..Display::default()
        };
        let surface = display.screen().unwrap().calibration.surface.unwrap();
        let columns = surface.columns as usize;
        let top = surface.positions_m[columns - 1][0] - surface.positions_m[0][0];
        let last = surface.positions_m.len() - 1;
        let bottom = surface.positions_m[last][0] - surface.positions_m[last + 1 - columns][0];
        assert!(top > bottom * 1.3, "top {top} bottom {bottom}");
    }

    #[test]
    fn rejects_values_that_would_fold_the_picture() {
        for display in [
            Display {
                keystone: Keystone {
                    horizontal: 0.9,
                    vertical: 0.0,
                },
                ..Display::default()
            },
            Display {
                width_m: 0.0,
                ..Display::default()
            },
            Display {
                barrel: f64::NAN,
                ..Display::default()
            },
            Display {
                waviness: Waviness {
                    waves_u: 40.0,
                    amplitude_m: 0.01,
                    ..Waviness::default()
                },
                ..Display::default()
            },
        ] {
            assert!(display.screen().is_err(), "{display:?}");
        }
    }

    #[test]
    fn a_display_adopted_from_a_hub_maps_stage_poses_into_the_hubs_frame() {
        let mut calibration = Display {
            width_m: 2.0,
            height_m: 1.0,
            ..Display::default()
        }
        .screen()
        .unwrap()
        .calibration;
        // A hub's screen is tilted against gravity and its top-left is not its origin.
        calibration.rotation = UnitQuaternion::from_euler_angles(0.3, -0.2, 0.1);
        let (dx, dy) = (0.4_f32, -0.25_f32);
        for point in &mut calibration.object_points {
            point.x += dx;
            point.y += dy;
        }
        calibration.homography *= Matrix3::new(1.0, 0.0, -dx, 0.0, 1.0, -dy, 0.0, 0.0, 1.0);
        let hub = SceneScreen::from_calibration(1, calibration);
        let adopted = Display::from_screen(&hub, true).unwrap();
        assert!(adopted.is_hub_screen());
        assert!((adopted.width_m - 2.0).abs() < 1e-4 && (adopted.height_m - 1.0).abs() < 1e-4);

        let staged = adopted.screen().unwrap();
        let upright = [0.0, 0.0, 0.0, 1.0];
        for uv in [[0.0, 0.0], [1.0, 0.0], [0.3, 0.8]] {
            let on_stage = staged.geometry().unwrap().point_at_uv(uv).unwrap();
            let (in_hub, _) = adopted.simulator_pose(on_stage, upright);
            let expected = hub.geometry().unwrap().point_at_uv(uv).unwrap();
            for axis in 0..3 {
                assert!((in_hub[axis] - expected[axis]).abs() < 1e-4, "{uv:?}");
            }
        }
        let (_, [x, y, z, w]) = adopted.simulator_pose([0.0, 0.0, -2.0], upright);
        let forward = UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z)) * Vector3::z();
        let normal = hub.geometry().unwrap().screen_to_world([0.0, 0.0, 1.0]);
        for axis in 0..3 {
            assert!((forward[axis] - normal[axis]).abs() < 1e-6);
        }

        let from_file = Display::from_screen(&hub, false).unwrap();
        assert_eq!(
            from_file.simulator_pose([1.0, 2.0, 3.0], upright),
            ([1.0, 2.0, 3.0], upright)
        );
    }
}
