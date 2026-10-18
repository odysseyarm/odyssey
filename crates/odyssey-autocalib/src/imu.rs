//! Gravity direction from the camera's IMU.
//!
//! The solver wants the camera frame rotated so that +Y is "down" and +Z is
//! the horizontal forward direction (the *aligned* frame). That makes the
//! screen's top edge horizontal for an upright screen and gives the hub a
//! gravity-referenced rotation. Heading about gravity cannot be observed
//! without a magnetometer; the estimator keeps the camera's own heading.

use std::time::Duration;

use nalgebra::{Matrix3, UnitQuaternion, Vector3, Vector4};

use crate::camera::ImuSample;

/// Madgwick's gradient-descent gain, the value the reference implementation
/// recommends for a 6-axis filter.
pub const MADGWICK_GAIN: f64 = 0.5;

/// Gaps longer than this restart integration instead of extrapolating the
/// gyro across a dropout.
const MAX_STEP: Duration = Duration::from_millis(500);

/// Maps camera axes (x right, y down, z forward) to the filter's axes (x
/// right, y forward, z up), the convention the Madgwick equations assume.
fn camera_to_filter() -> Matrix3<f64> {
    Matrix3::new(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0)
}

/// Madgwick's IMU-only orientation filter. The quaternion rotates filter-frame
/// vectors into the earth frame whose z axis points up.
#[derive(Debug, Clone)]
pub struct Madgwick {
    q: Vector4<f64>, // w, x, y, z
    gain: f64,
}

impl Madgwick {
    /// Starts level with identity orientation.
    pub fn new(gain: f64) -> Self {
        Self { q: Vector4::new(1.0, 0.0, 0.0, 0.0), gain }
    }

    /// Starts from the orientation that points "up" at the measured specific
    /// force, so the filter does not spend its first second converging.
    pub fn from_accelerometer(gain: f64, accel: &Vector3<f64>) -> Self {
        let q = UnitQuaternion::rotation_between(accel, &Vector3::z()).unwrap_or_default();
        Self { q: Vector4::new(q.w, q.i, q.j, q.k), gain }
    }

    pub fn orientation(&self) -> UnitQuaternion<f64> {
        UnitQuaternion::from_quaternion(nalgebra::Quaternion::new(self.q[0], self.q[1], self.q[2], self.q[3]))
    }

    /// Integrates `gyro` (rad/s) over `dt` seconds and corrects toward `accel`.
    /// Real inter-sample intervals matter: a fixed step mis-scales the gyro
    /// whenever the sample rate differs from the assumed one.
    pub fn update(&mut self, gyro: &Vector3<f64>, accel: &Vector3<f64>, dt: f64) {
        let [q0, q1, q2, q3] = [self.q[0], self.q[1], self.q[2], self.q[3]];
        let mut q_dot = Vector4::new(
            0.5 * (-q1 * gyro.x - q2 * gyro.y - q3 * gyro.z),
            0.5 * (q0 * gyro.x + q2 * gyro.z - q3 * gyro.y),
            0.5 * (q0 * gyro.y - q1 * gyro.z + q3 * gyro.x),
            0.5 * (q0 * gyro.z + q1 * gyro.y - q2 * gyro.x),
        );

        let norm = accel.norm();
        if norm > 1e-9 {
            let a = accel / norm;
            let f = Vector3::new(
                2.0 * (q1 * q3 - q0 * q2) - a.x,
                2.0 * (q0 * q1 + q2 * q3) - a.y,
                2.0 * (0.5 - q1 * q1 - q2 * q2) - a.z,
            );
            let jacobian = nalgebra::Matrix3x4::new(
                -2.0 * q2, 2.0 * q3, -2.0 * q0, 2.0 * q1,
                2.0 * q1, 2.0 * q0, 2.0 * q3, 2.0 * q2,
                0.0, -4.0 * q1, -4.0 * q2, 0.0,
            );
            let gradient = jacobian.transpose() * f;
            let gn = gradient.norm();
            if gn > 1e-12 {
                q_dot -= gradient * (self.gain / gn);
            }
        }

        self.q += q_dot * dt;
        self.q /= self.q.norm();
    }
}

/// Camera tilt relative to gravity, in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tilt {
    /// Positive when the camera looks down.
    pub pitch_deg: f64,
    /// Rotation about the optical axis; positive turns the image clockwise.
    pub roll_deg: f64,
}

/// Consumes IMU samples and reports the camera-to-aligned rotation.
#[derive(Debug, Clone, Default)]
pub struct GravityEstimator {
    filter: Option<Madgwick>,
    last: Option<Duration>,
    samples: usize,
}

impl GravityEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, sample: &ImuSample) {
        let to_filter = camera_to_filter();
        let accel = to_filter * sample.accel;
        let gyro = to_filter * sample.gyro;
        self.samples += 1;

        let Some(filter) = self.filter.as_mut() else {
            if accel.norm() > 1e-6 {
                self.filter = Some(Madgwick::from_accelerometer(MADGWICK_GAIN, &accel));
                self.last = Some(sample.timestamp);
            }
            return;
        };

        let dt = self.last.map(|last| sample.timestamp.saturating_sub(last));
        self.last = Some(sample.timestamp);
        match dt {
            Some(dt) if !dt.is_zero() && dt <= MAX_STEP => filter.update(&gyro, &accel, dt.as_secs_f64()),
            _ => {}
        }
    }

    pub fn extend<'a>(&mut self, samples: impl IntoIterator<Item = &'a ImuSample>) {
        for s in samples {
            self.push(s);
        }
    }

    /// Samples consumed since creation, including ones skipped for lack of a
    /// usable accelerometer reading.
    pub fn sample_count(&self) -> usize {
        self.samples
    }

    /// Whether an orientation has been established.
    pub fn is_ready(&self) -> bool {
        self.filter.is_some()
    }

    /// Rotation taking camera-frame vectors to the aligned frame. Identity
    /// without IMU data, which assumes an upright camera.
    pub fn camera_to_aligned(&self) -> Matrix3<f64> {
        let Some(filter) = &self.filter else { return Matrix3::identity() };
        let m = camera_to_filter();
        m.transpose() * filter.orientation().to_rotation_matrix().into_inner() * m
    }

    /// Unit gravity direction in the camera frame, when known.
    pub fn down_in_camera(&self) -> Option<Vector3<f64>> {
        self.filter.as_ref()?;
        Some(self.camera_to_aligned().transpose() * Vector3::y())
    }

    pub fn tilt(&self) -> Option<Tilt> {
        let d = self.down_in_camera()?;
        Some(Tilt {
            pitch_deg: d.z.atan2(d.x.hypot(d.y)).to_degrees(),
            roll_deg: d.x.atan2(d.y).to_degrees(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rest_sample(t_ms: u64, down: Vector3<f64>) -> ImuSample {
        ImuSample { timestamp: Duration::from_millis(t_ms), gyro: Vector3::zeros(), accel: -down.normalize() * 9.81 }
    }

    #[test]
    fn no_samples_means_identity() {
        let g = GravityEstimator::new();
        assert_eq!(g.camera_to_aligned(), Matrix3::identity());
        assert!(g.tilt().is_none());
    }

    #[test]
    fn aligned_frame_puts_gravity_on_plus_y() {
        // Camera pitched 20 degrees down and rolled 10 degrees.
        let down = Vector3::new(0.17, 0.92, 0.34);
        let mut g = GravityEstimator::new();
        for i in 0..200 {
            g.push(&rest_sample(i * 5, down));
        }
        let a = g.camera_to_aligned();
        assert!((a * down.normalize() - Vector3::y()).norm() < 1e-6);
        assert!((a * a.transpose() - Matrix3::identity()).norm() < 1e-9);
        assert!((a.determinant() - 1.0).abs() < 1e-9);
        let tilt = g.tilt().unwrap();
        assert!((tilt.pitch_deg - (0.34f64).atan2(0.17f64.hypot(0.92)).to_degrees()).abs() < 1e-6);
    }

    #[test]
    fn upright_camera_is_identity() {
        let mut g = GravityEstimator::new();
        for i in 0..50 {
            g.push(&rest_sample(i * 10, Vector3::y()));
        }
        assert!((g.camera_to_aligned() - Matrix3::identity()).norm() < 1e-9);
    }

    #[test]
    fn filter_tracks_gyro_with_real_sample_interval() {
        // Camera rolls about its optical axis at 0.5 rad/s for one second,
        // sampled at 50 Hz; accelerometer shows the matching gravity direction.
        let rate = 0.5;
        let mut g = GravityEstimator::new();
        g.push(&rest_sample(0, Vector3::y()));
        for i in 1..=50u64 {
            let t = i as f64 * 0.02;
            // Rolling the camera by +angle about z moves gravity toward +x
            // in the camera frame.
            let down = Vector3::new((rate * t).sin(), (rate * t).cos(), 0.0);
            g.push(&ImuSample {
                timestamp: Duration::from_millis(i * 20),
                gyro: Vector3::new(0.0, 0.0, -rate),
                accel: -down * 9.81,
            });
        }
        let tilt = g.tilt().unwrap();
        assert!((tilt.roll_deg - rate.to_degrees()).abs() < 3.0, "roll {}", tilt.roll_deg);
    }
}
