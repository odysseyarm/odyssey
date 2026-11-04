use nalgebra::{Point2, Vector3};
use odyssey_protocol_macros::protocol_type;

use crate::{AccelConfig, GyroConfig};

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotData {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub area: u16,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub cx: u16,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub cy: u16,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub avg_brightness: u8,
    #[cfg_attr(feature = "minicbor", n(4))]
    pub max_brightness: u8,
    #[cfg_attr(feature = "minicbor", n(5))]
    pub range: u8,
    #[cfg_attr(feature = "minicbor", n(6))]
    pub radius: u8,
    #[cfg_attr(feature = "minicbor", n(7))]
    pub boundary_left: u8,
    #[cfg_attr(feature = "minicbor", n(8))]
    pub boundary_right: u8,
    #[cfg_attr(feature = "minicbor", n(9))]
    pub boundary_up: u8,
    #[cfg_attr(feature = "minicbor", n(10))]
    pub boundary_down: u8,
    #[cfg_attr(feature = "minicbor", n(11))]
    pub aspect_ratio: u8,
    #[cfg_attr(feature = "minicbor", n(12))]
    pub vx: u8,
    #[cfg_attr(feature = "minicbor", n(13))]
    pub vy: u8,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub mot_data_nf: [MotData; 16],
    #[cfg_attr(feature = "minicbor", n(2))]
    pub mot_data_wf: [MotData; 16],
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CombinedMarkersReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    pub nf_points: [Point2<u16>; 16],
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    pub wf_points: [Point2<u16>; 16],
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PocMarkersReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    pub points: [Point2<u16>; 16],
}

impl From<PocMarkersReport> for CombinedMarkersReport {
    fn from(t: PocMarkersReport) -> Self {
        Self {
            nf_points: t.points,
            wf_points: Default::default(),
        }
    }
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AccelReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    pub accel: Vector3<f32>,
    #[cfg_attr(feature = "minicbor", n(2))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    pub gyro: Vector3<f32>,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpactReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatteryReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub percent: u8,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub charging: bool,
}

#[cfg(feature = "pyo3")]
#[pyo3::pymethods]
impl CombinedMarkersReport {
    #[getter]
    fn nf_points(&self) -> [[u16; 2]; 16] {
        self.nf_points.map(|p| [p.x, p.y])
    }
    #[getter]
    fn wf_points(&self) -> [[u16; 2]; 16] {
        self.wf_points.map(|p| [p.x, p.y])
    }
}

impl AccelReport {
    pub fn corrected_accel(&self, accel_config: &AccelConfig) -> Vector3<f32> {
        Vector3::new(
            (self.accel.x - accel_config.b_x) / accel_config.s_x,
            (self.accel.y - accel_config.b_y) / accel_config.s_y,
            (self.accel.z - accel_config.b_z) / accel_config.s_z,
        )
    }

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
