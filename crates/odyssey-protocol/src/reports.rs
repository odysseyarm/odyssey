use nalgebra::{Point2, Vector3};
use odyssey_protocol_macros::protocol_type;

use crate::{AccelConfig, GyroConfig};

/// What the tracking sensor sees: marker positions on the sensor's report grid
/// ([`Self::UNITS_PER_PX`] units per native pixel), with empty slots at the origin.
///
/// Since: Odyssey Desktop 0.7.5.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkersReport {
    /// Since: Odyssey Desktop 0.7.5.
    #[cfg_attr(feature = "minicbor", n(0))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    #[cfg_attr(feature = "schema", postcard_type = "[[u16; 2]; 16]")]
    pub points: [Point2<u16>; 16],
}

impl MarkersReport {
    /// Report units per native sensor pixel: the PAG7665QN reports 1/64 px. The hub passes it
    /// to the calibration estimator, and marker hints use the same grid.
    ///
    /// Since: Unreleased.
    pub const UNITS_PER_PX: f64 = 64.0;
}

/// Motion since the previous report: mean acceleration (m/s²) and mean rotation rate (rad/s)
/// over the interval ending at `timestamp` (µs, wrapping). See [`ImuIntegrator`].
///
/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AccelReport {
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    #[cfg_attr(feature = "schema", postcard_type = "[f32; 3]")]
    pub accel: Vector3<f32>,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(2))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    #[cfg_attr(feature = "schema", postcard_type = "[f32; 3]")]
    pub gyro: Vector3<f32>,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpactReport {
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatteryReport {
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub percent: u8,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub charging: bool,
}

impl AccelReport {
    /// Since: Odyssey Desktop 0.6.0.
    pub fn corrected_accel(&self, accel_config: &AccelConfig) -> Vector3<f32> {
        Vector3::new(
            (self.accel.x - accel_config.b_x) / accel_config.s_x,
            (self.accel.y - accel_config.b_y) / accel_config.s_y,
            (self.accel.z - accel_config.b_z) / accel_config.s_z,
        )
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn corrected_gyro(&self, gyro_config: &GyroConfig) -> Vector3<f32> {
        Vector3::new(
            self.gyro.x - gyro_config.b_x,
            self.gyro.y - gyro_config.b_y,
            self.gyro.z - gyro_config.b_z,
        )
    }
}

/// Sums IMU samples that arrive faster than they are reported.
///
/// A report carries the mean rotation rate and the mean acceleration since the previous one,
/// the acceleration in the frame the unit had when that interval began. Integrating a report
/// over its interval as if it were constant recovers the summed rotation and velocity change.
#[derive(Clone, Copy, Debug, Default)]
pub struct ImuIntegrator {
    previous: Option<u32>,
    micros: u32,
    rotation: Vector3<f32>,
    velocity: Vector3<f32>,
}

impl ImuIntegrator {
    /// Adds the sample taken at `timestamp` (µs, wrapping), held since the sample before it.
    /// Returns a report once the interval is as close to `1 / report_hz` as whole samples allow.
    pub fn push(
        &mut self,
        timestamp: u32,
        accel: Vector3<f32>,
        gyro: Vector3<f32>,
        report_hz: u16,
    ) -> Option<AccelReport> {
        let step_micros = timestamp.wrapping_sub(self.previous.replace(timestamp)?);
        let dt = step_micros as f32 * 1e-6;
        let step = gyro * dt;
        // Rotate the acceleration into the interval's first frame, to second order in the
        // rotation vector reached halfway through this step.
        let turn = self.rotation + step * 0.5;
        let turned = turn.cross(&accel);
        self.velocity += (accel + turned + turn.cross(&turned) * 0.5) * dt;
        // Rotations do not commute; the cross term keeps their order.
        self.rotation += step + self.rotation.cross(&step) * 0.5;
        self.micros = self.micros.saturating_add(step_micros);
        if self.micros.saturating_add(step_micros / 2) < 1_000_000 / u32::from(report_hz.max(1)) {
            return None;
        }
        let seconds = self.micros as f32 * 1e-6;
        let report = AccelReport {
            timestamp,
            accel: self.velocity / seconds,
            gyro: self.rotation / seconds,
        };
        *self = Self {
            previous: Some(timestamp),
            ..Self::default()
        };
        Some(report)
    }
}

#[cfg(feature = "pyo3")]
#[pyo3::pymethods]
impl AccelReport {
    #[getter]
    fn timestamp(&self) -> u32 {
        self.timestamp
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use nalgebra::{UnitQuaternion, Vector3};

    use super::ImuIntegrator;

    #[test]
    fn reports_follow_the_requested_rate_as_closely_as_samples_allow() {
        for (sample_micros, report_hz, samples_per_report) in [
            (1000, 100, 10),
            (1000, 200, 5),
            (8333, 100, 1),
            (10_000, 100, 1),
            (1000, 0, 1000),
        ] {
            let mut integrator = ImuIntegrator::default();
            let mut pushed = 0;
            let mut timestamp = u32::MAX - 3 * sample_micros;
            assert!(
                integrator
                    .push(timestamp, Vector3::zeros(), Vector3::zeros(), report_hz)
                    .is_none()
            );
            let report = loop {
                timestamp = timestamp.wrapping_add(sample_micros);
                pushed += 1;
                if let Some(report) =
                    integrator.push(timestamp, Vector3::zeros(), Vector3::zeros(), report_hz)
                {
                    break report;
                }
            };
            assert_eq!(
                (pushed, report.timestamp),
                (samples_per_report, timestamp),
                "{sample_micros} µs samples at {report_hz} Hz"
            );
        }
    }

    #[test]
    fn a_report_carries_the_rotation_and_velocity_change_of_a_fast_turn() {
        const SAMPLE_S: f32 = 0.001;
        const SUBSTEPS: usize = 200;
        let mut integrator = ImuIntegrator::default();
        integrator.push(0, Vector3::zeros(), Vector3::zeros(), 100);
        let mut orientation = UnitQuaternion::identity();
        let mut velocity = Vector3::zeros();
        let mut naive_accel = Vector3::zeros();
        let mut report = None;
        for sample in 1..=10u32 {
            let phase = sample as f32;
            // About 600°/s with a moving axis, under a constant world-frame pull.
            let gyro = Vector3::new(8.0 - phase, 2.0 * phase - 6.0, 5.0);
            let accel = orientation.inverse() * Vector3::new(0.0, 9.81, 3.0);
            naive_accel += accel / 10.0;
            for _ in 0..SUBSTEPS {
                let dt = SAMPLE_S / SUBSTEPS as f32;
                velocity += orientation * accel * dt;
                orientation *= UnitQuaternion::from_scaled_axis(gyro * dt);
            }
            report = integrator.push(sample * 1000, accel, gyro, 100);
        }
        let report = report.expect("ten 1 kHz samples fill a 100 Hz report");
        let rotation_error = (report.gyro * 0.01 - orientation.scaled_axis()).norm();
        let velocity_error = (report.accel * 0.01 - velocity).norm();
        let naive_error = (naive_accel * 0.01 - velocity).norm();
        assert!(
            rotation_error < 1e-5,
            "rotation off by {rotation_error} rad"
        );
        assert!(
            velocity_error < 1e-4 && velocity_error * 10.0 < naive_error,
            "velocity off by {velocity_error} m/s, plain averaging by {naive_error} m/s"
        );
    }
}
