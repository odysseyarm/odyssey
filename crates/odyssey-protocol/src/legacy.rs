//! Protocol that current units do not use: the payloads of the two-sensor
//! PAJ7025 units, and the packed binary packet format from before postcard
//! and CBOR. Current firmware reports [`MarkersReport`] and refuses or ignores
//! the packets that carry these payloads.
//!
//! The packets keep their place in [`crate::PacketData`] and
//! [`crate::PacketType`], where their names start with `Legacy`: the position
//! and the IDs are the wire format.

mod codec;

pub use codec::{Parse, Serialize};
use nalgebra::Point2;
use odyssey_protocol_macros::protocol_type;

use crate::MarkersReport;

#[protocol_type]
#[repr(u8)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    #[cfg_attr(feature = "minicbor", n(0))]
    Object,
    #[cfg_attr(feature = "minicbor", n(1))]
    Image,
}

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

/// Markers of the near-field and the wide-field sensor.
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

impl From<MarkersReport> for CombinedMarkersReport {
    fn from(t: MarkersReport) -> Self {
        Self {
            nf_points: t.points,
            wf_points: Default::default(),
        }
    }
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
