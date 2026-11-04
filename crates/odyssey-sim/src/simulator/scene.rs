use nalgebra::{Matrix3, Point2, Point3, UnitQuaternion, Vector3};
use odyssey_common::ScreenCalibration;
use odyssey_protocol::{GeneralConfig, PocMarkersReport};

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

/// Screen ID 0 for an isolated simulator configuration. No user calibration files
/// should be overwritten. Its physical XY plane is viewed from two meters away.
pub fn synthetic_screen_calibration() -> ScreenCalibration<f32> {
    ScreenCalibration {
        surface: None,
        rotation: UnitQuaternion::identity(),
        homography: Matrix3::new(1.0 / WIDTH, 0.0, 0.0, 0.0, 1.0 / HEIGHT, 0.0, 0.0, 0.0, 1.0),
        object_points: MARKERS.map(|[x, y]| Point3::new(x * WIDTH, y * HEIGHT, 0.0)),
    }
}

pub(crate) fn markers(
    aim: [f32; 2],
    orientation: &UnitQuaternion<f32>,
    config: &GeneralConfig,
) -> PocMarkersReport {
    let mut points = [Point2::origin(); 16];
    let GeneralConfig::CameraModelNf(camera) = config else {
        return PocMarkersReport { points };
    };
    // +X right, +Y down, +Z optical forward. Aim translates the camera;
    // integrated angular velocity rotates the same camera about its center.
    for (output, [x, y]) in points.iter_mut().zip(MARKERS) {
        let local = orientation.inverse_transform_vector(&Vector3::new(
            (x - aim[0]) * WIDTH,
            (y - aim[1]) * HEIGHT,
            2.0,
        ));
        if local.z <= 0.0 {
            continue;
        }
        let nx = local.x / local.z;
        let ny = local.y / local.z;
        let r2 = nx * nx + ny * ny;
        let [k1, k2, p1, p2, k3] = camera.distortion;
        let radial = 1.0 + k1 * r2 + k2 * r2 * r2 + k3 * r2 * r2 * r2;
        let dx = nx * radial + 2.0 * p1 * nx * ny + p2 * (r2 + 2.0 * nx * nx);
        let dy = ny * radial + p1 * (r2 + 2.0 * ny * ny) + 2.0 * p2 * nx * ny;
        let u = camera.k[0] * dx + camera.k[3] * dy + camera.k[6];
        let v = camera.k[4] * dy + camera.k[7];
        if u.is_finite()
            && v.is_finite()
            && (1.0..=639.0).contains(&u)
            && (1.0..=479.0).contains(&v)
        {
            *output = Point2::new(u.round() as u16, v.round() as u16);
        }
    }
    PocMarkersReport { points }
}
