//! What the synthetic camera looks at, and the calibration that scene implies.

use nalgebra::{Matrix3, Point2, Point3, Rotation3, UnitQuaternion, Vector2, Vector3};
use odyssey_common::ScreenCalibration;

use crate::camera::{CameraGeometry, Extrinsics, Intrinsics};
use crate::pattern::Pattern;
use crate::solve::METERS_PER_INCH;

/// Where the simulated markers sit, as fractions of the panel from its top-left corner: the
/// original autocalib's pattern, three along the top edge and three along the bottom. It is
/// also in the order the tracker needs (see `crate::placement`).
pub const SIMULATED_MARKERS: [[f64; 2]; 6] = [
    [0.2, 1.0],
    [0.2, 0.0],
    [0.8, 0.0],
    [0.5, 0.0],
    [0.5, 1.0],
    [0.8, 1.0],
];

/// Everything that defines the synthetic scene. Fields are public: build one
/// from [`Default`] and change what a scenario needs.
#[derive(Debug, Clone)]
pub struct SyntheticScene {
    /// Physical width and height of the screen in meters.
    pub screen_size_m: [f64; 2],
    /// Center of the screen in the world frame.
    pub screen_center_m: Vector3<f64>,
    /// How much a display overscans: the image is scaled up about its center
    /// by `1 + overscan_fraction` while the panel keeps its size, so the
    /// border of the image is cut off. The solver measures the image, not the
    /// panel.
    pub overscan_fraction: f64,
    /// Multiplicative bias on every depth value (`0.01` reads 1% too far),
    /// standing in for depth scale error.
    pub depth_scale_error: f64,
    /// Rotation of the screen about the vertical axis, degrees.
    pub screen_yaw_deg: f64,
    /// Backward lean of the screen's top edge, degrees.
    pub screen_pitch_deg: f64,
    /// Camera rotation about the vertical axis, degrees.
    pub camera_yaw_deg: f64,
    /// Camera pitch in degrees; positive looks down.
    pub camera_pitch_deg: f64,
    /// Camera rotation about its optical axis in degrees; positive lowers the
    /// camera's right side.
    pub camera_roll_deg: f64,
    pub geometry: CameraGeometry,
    /// Distance in front of the screen plane at which the markers sit.
    pub marker_standoff_m: f64,
    /// Standard deviation of per-pixel, per-frame depth noise, meters.
    pub depth_noise_m: f64,
    /// Fraction of depth pixels that drop out independently in every frame.
    pub depth_hole_fraction: f64,
    /// Fraction of depth pixels that read a persistent wrong value.
    pub depth_outlier_fraction: f64,
    /// Rectangle `[u0, v0, u1, v1]` of the screen, in unit coordinates, where
    /// the depth sensor returns nothing.
    pub depth_dropout_region: Option<[f64; 4]>,
    /// Standard deviation of color noise in 8-bit levels.
    pub color_noise: f64,
    /// Extra bright spots in the infrared image, in infrared pixels.
    pub ir_clutter: Vec<Point2<f64>>,
    /// Whether the camera reports IMU samples.
    pub imu_enabled: bool,
    /// Whether the screen shows the dot pattern. Without it the color image
    /// shows a plain black screen.
    pub show_pattern: bool,
    /// Multiplier on the pattern's dot radii; see [`Pattern::with_dot_scale`].
    pub dot_scale: f64,
    /// Sleep one frame interval per frame, as a real camera would.
    pub realtime: bool,
    pub seed: u64,
}

impl Default for SyntheticScene {
    /// A 55-inch 16:9 screen two meters away with a slight yaw, seen by an
    /// upright camera.
    fn default() -> Self {
        let diagonal = 55.0 * METERS_PER_INCH;
        let aspect: f64 = 16.0 / 9.0;
        let height = diagonal / (aspect * aspect + 1.0).sqrt();
        let color = Intrinsics {
            width: 1280,
            height: 720,
            fx: 920.0,
            fy: 922.0,
            cx: 640.3,
            cy: 359.1,
            k: [-0.05, 0.02, -0.003, 0.0, 0.0, 0.0],
            p: [0.0006, -0.0004],
        };
        let depth = Intrinsics {
            width: 640,
            height: 400,
            fx: 455.0,
            fy: 455.0,
            cx: 320.4,
            cy: 199.2,
            k: [-0.012, 0.004, 0.0, 0.0, 0.0, 0.0],
            p: [0.0, 0.0],
        };
        let tilt = Rotation3::from_euler_angles(0.0035, -0.005, 0.0015);
        Self {
            screen_size_m: [aspect * height, height],
            screen_center_m: Vector3::new(0.0, 0.0, 2.0),
            screen_yaw_deg: 12.0,
            overscan_fraction: 0.0,
            depth_scale_error: 0.0,
            screen_pitch_deg: 0.0,
            camera_yaw_deg: 0.0,
            camera_pitch_deg: 0.0,
            camera_roll_deg: 0.0,
            geometry: CameraGeometry {
                color,
                depth,
                ir: depth,
                color_to_depth: Extrinsics {
                    rotation: tilt.into_inner(),
                    translation: Vector3::new(-0.032, 0.001, -0.002),
                },
                ir_to_depth: Extrinsics::identity(),
            },
            marker_standoff_m: 0.0,
            depth_noise_m: 0.002,
            depth_hole_fraction: 0.0,
            depth_outlier_fraction: 0.0,
            depth_dropout_region: None,
            color_noise: 1.5,
            ir_clutter: Vec::new(),
            imu_enabled: true,
            show_pattern: true,
            dot_scale: 1.0,
            realtime: false,
            seed: 1,
        }
    }
}

/// The calibration the solver should recover from a [`SyntheticScene`].
#[derive(Debug, Clone, Copy)]
pub struct Truth {
    /// Takes color-camera coordinates to the physical screen frame.
    pub screen_from_camera: Extrinsics,
    /// Width and height of the displayed image: what the calibration must
    /// recover. Equals `panel_m` without overscan.
    pub size_m: [f64; 2],
    /// Width and height of the physical panel: what an EDID reports.
    pub panel_m: [f64; 2],
    /// Markers in the physical screen frame, whose origin is the image's
    /// top-left corner, in layout order.
    pub markers_m: [Point3<f64>; 6],
}

impl Truth {
    /// Screen corners (top left, top right, bottom right, bottom left) in
    /// the physical screen frame.
    pub fn corners_m(&self) -> [Point2<f64>; 4] {
        let [w, h] = self.size_m;
        [
            Point2::new(0.0, 0.0),
            Point2::new(w, 0.0),
            Point2::new(w, h),
            Point2::new(0.0, h),
        ]
    }

    pub fn diagonal_in(&self) -> f64 {
        self.size_m[0].hypot(self.size_m[1]) / METERS_PER_INCH
    }

    /// The calibration a perfect solver returns when the camera-to-aligned
    /// rotation is `camera_to_aligned`.
    pub fn calibration(&self, camera_to_aligned: &Matrix3<f64>) -> ScreenCalibration<f32> {
        let screen_from_aligned = self.screen_from_camera.rotation * camera_to_aligned.transpose();
        let rotation = UnitQuaternion::from_rotation_matrix(&Rotation3::from_matrix_unchecked(
            screen_from_aligned,
        ));
        let [w, h] = self.size_m;
        ScreenCalibration {
            rotation: rotation.cast(),
            homography: Matrix3::new(1.0 / w, 0.0, 0.0, 0.0, 1.0 / h, 0.0, 0.0, 0.0, 1.0).cast(),
            object_points: self.markers_m.map(nalgebra::OPoint::cast),
        }
    }
}

/// The scene's rigid transforms.
pub(super) struct Pose {
    /// Camera frame to world frame.
    pub(super) world_from_camera: Matrix3<f64>,
    pub(super) screen_from_camera: Extrinsics,
}

impl SyntheticScene {
    pub fn aspect(&self) -> f64 {
        self.screen_size_m[0] / self.screen_size_m[1]
    }

    /// The pattern the screen shows.
    pub fn pattern(&self) -> Pattern {
        Pattern::with_dot_scale(self.aspect(), self.dot_scale)
    }

    pub(super) fn pose(&self) -> Pose {
        let rot = |axis: Vector3<f64>, deg: f64| {
            Rotation3::from_axis_angle(&nalgebra::Unit::new_normalize(axis), deg.to_radians())
        };
        let world_from_camera = (rot(Vector3::y(), self.camera_yaw_deg)
            * rot(Vector3::x(), -self.camera_pitch_deg)
            * rot(Vector3::z(), self.camera_roll_deg))
        .into_inner();
        let world_from_screen = (rot(Vector3::y(), self.screen_yaw_deg)
            * rot(Vector3::x(), self.screen_pitch_deg))
        .into_inner();
        let [w, h] = self.screen_size_m;
        let top_left_world =
            self.screen_center_m - world_from_screen * Vector3::new(w / 2.0, h / 2.0, 0.0);
        let screen_from_world = world_from_screen.transpose();
        Pose {
            world_from_camera,
            screen_from_camera: Extrinsics {
                rotation: screen_from_world * world_from_camera,
                translation: -(screen_from_world * top_left_world),
            },
        }
    }

    pub fn truth(&self) -> Truth {
        let [w, h] = self.screen_size_m;
        let [iw, ih] = self.image_extent_m();
        let origin = self.image_origin_m();
        Truth {
            screen_from_camera: self.pose().screen_from_camera,
            size_m: [iw, ih],
            panel_m: self.screen_size_m,
            // The markers sit on the panel; the frame starts at the image's corner.
            markers_m: SIMULATED_MARKERS
                .map(|[u, v]| Point3::new(u * w - origin.x, v * h - origin.y, 0.0)),
        }
    }

    /// Width and height in meters of the displayed image, which exceeds the
    /// panel by the overscan.
    pub fn image_extent_m(&self) -> [f64; 2] {
        let scale = 1.0 + self.overscan_fraction;
        [self.screen_size_m[0] * scale, self.screen_size_m[1] * scale]
    }

    /// Position of the image's top-left corner in panel coordinates; negative
    /// under overscan, since the image extends past the panel.
    pub fn image_origin_m(&self) -> Vector2<f64> {
        Vector2::new(
            -self.screen_size_m[0] * self.overscan_fraction / 2.0,
            -self.screen_size_m[1] * self.overscan_fraction / 2.0,
        )
    }

    /// Gravity direction in the camera frame.
    pub fn down_in_camera(&self) -> Vector3<f64> {
        self.pose().world_from_camera.transpose() * Vector3::y()
    }

    /// The camera-to-aligned rotation an ideal gravity estimate yields: the
    /// smallest rotation that points gravity along `+y`.
    pub fn camera_to_aligned(&self) -> Matrix3<f64> {
        UnitQuaternion::rotation_between(&self.down_in_camera(), &Vector3::y())
            .unwrap_or_default()
            .to_rotation_matrix()
            .into_inner()
    }
}
