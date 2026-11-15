use nalgebra::{Point2, Vector3};
use odyssey_protocol_macros::protocol_type;

use crate::{AccelConfig, GyroConfig};

/// What the tracking sensor sees: marker positions in its pixel grid, with
/// empty slots at the origin.
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

#[cfg(feature = "pyo3")]
#[pyo3::pymethods]
impl AccelReport {
    #[getter]
    fn timestamp(&self) -> u32 {
        self.timestamp
    }
}
