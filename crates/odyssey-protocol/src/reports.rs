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

/// Marker positions encoded densely while retaining original slot positions.
///
/// `mask` bit `i` marks occupied slot `i`; `points` stores occupied positions in
/// ascending slot order.
///
/// See the [crate documentation](crate) for the report transport design.
///
/// Since: Unreleased.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SparseMarkersReport {
    /// Occupied marker slots, indexed from zero.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub mask: u16,
    /// Occupied points ordered by their original slot indices.
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    #[cfg_attr(feature = "schema", postcard_type = "crate::List<[u16; 2], 16>")]
    pub points: crate::List<Point2<u16>, 16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SparseMarkersError;

impl core::fmt::Display for SparseMarkersError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("marker occupancy mask and point count disagree")
    }
}

impl core::error::Error for SparseMarkersError {}

impl SparseMarkersReport {
    /// Reconstructs dense report, preserving marker slot positions.
    ///
    /// # Errors
    /// Returns [`SparseMarkersError`] if the point count differs from the occupancy mask.
    pub fn into_markers_report(self) -> Result<MarkersReport, SparseMarkersError> {
        if self.mask.count_ones() as usize != self.points.len() {
            return Err(SparseMarkersError);
        }
        let mut points = [Point2::origin(); 16];
        let mut occupied = self.points.iter();
        for (slot, point) in points.iter_mut().enumerate() {
            if self.mask & (1 << slot) != 0 {
                *point = *occupied.next().ok_or(SparseMarkersError)?;
            }
        }
        Ok(MarkersReport { points })
    }
}

impl TryFrom<MarkersReport> for SparseMarkersReport {
    type Error = SparseMarkersError;

    fn try_from(report: MarkersReport) -> Result<Self, Self::Error> {
        let mut mask = 0u16;
        let mut points = heapless::Vec::new();
        for (slot, point) in report.points.into_iter().enumerate() {
            if point != Point2::origin() {
                mask |= 1 << slot;
                points.push(point).map_err(|_point| SparseMarkersError)?;
            }
        }
        Ok(Self {
            mask,
            points: points.into(),
        })
    }
}

/// Bounded sequence of independent acceleration reports.
///
/// See the [crate documentation](crate) for the report transport design.
///
/// Since: Unreleased.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Debug)]
pub struct AccelBatch {
    /// Reports in measurement-time order.
    #[cfg_attr(feature = "minicbor", n(0))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    pub reports: crate::List<AccelReport, 8>,
}

impl AccelBatch {
    /// Iterates reports independently, with original timestamps and float values.
    pub fn iter(&self) -> impl Iterator<Item = AccelReport> + '_ {
        self.reports.iter().copied()
    }
}

/// Motion since the previous report: mean acceleration (m/s²) and mean rotation rate (rad/s)
/// over the interval ending at `timestamp` (µs, wrapping).
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

#[cfg(feature = "pyo3")]
#[pyo3::pymethods]
impl AccelReport {
    #[getter]
    fn timestamp(&self) -> u32 {
        self.timestamp
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::{Point2, Vector3};

    use super::{AccelBatch, AccelReport, MarkersReport, SparseMarkersError, SparseMarkersReport};
    use crate::List;

    #[test]
    fn sparse_marker_conversion_preserves_slots_and_rejects_wrong_count() {
        let report = MarkersReport {
            points: core::array::from_fn(|slot| {
                if slot == 2 || slot == 14 {
                    Point2::new(100 + slot as u16, slot as u16)
                } else {
                    Point2::origin()
                }
            }),
        };
        let sparse = SparseMarkersReport::try_from(report).expect("valid sparse report");
        assert_eq!(sparse.mask, (1 << 2) | (1 << 14));
        assert_eq!(sparse.points.len(), 2);
        assert_eq!(
            sparse.into_markers_report().expect("reconstruct").points,
            report.points
        );
        assert_eq!(
            SparseMarkersReport {
                mask: 1,
                points: List::default()
            }
            .into_markers_report(),
            Err(SparseMarkersError)
        );
        let invalid = heapless::Vec::from_slice(&[Point2::new(1, 2)]).expect("fits");
        assert_eq!(
            SparseMarkersReport {
                mask: 0,
                points: invalid.into()
            }
            .into_markers_report(),
            Err(SparseMarkersError)
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn postcard_roundtrip_preserves_report_float_bits() {
        let report = AccelReport {
            timestamp: 0xdead_beef,
            accel: Vector3::new(f32::from_bits(0x7fc0_1234), -0.0, f32::from_bits(1)),
            gyro: Vector3::new(f32::from_bits(0xffc0_5678), 3.0, 4.0),
        };
        let batch = AccelBatch {
            reports: heapless::Vec::from_slice(&[report])
                .expect("one report fits")
                .into(),
        };
        let packet = crate::Packet {
            data: crate::PacketData::AccelBatch(batch),
            id: 91,
        };
        let bytes = postcard::to_stdvec(&packet).expect("serialize packet");
        let decoded: crate::Packet = postcard::from_bytes(&bytes).expect("deserialize packet");
        let crate::PacketData::AccelBatch(decoded) = decoded.data else {
            panic!("batch variant preserved");
        };
        let report = decoded.iter().next().expect("batch has report");
        assert_eq!(report.timestamp, 0xdead_beef);
        assert_eq!(report.accel.x.to_bits(), 0x7fc0_1234);
        assert_eq!(report.accel.y.to_bits(), 0x8000_0000);
        assert_eq!(report.accel.z.to_bits(), 1);
        assert_eq!(report.gyro.x.to_bits(), 0xffc0_5678);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn full_batch_fits_one_pdu_and_decodes_after_an_earlier_batch_is_lost() {
        let report = AccelReport {
            timestamp: u32::MAX,
            accel: Vector3::new(-0.0, f32::from_bits(0x7fc0_1234), 1.0),
            gyro: Vector3::new(2.0, 3.0, 4.0),
        };
        let earlier = AccelBatch {
            reports: heapless::Vec::from_slice(&[report])
                .expect("one report fits")
                .into(),
        };
        let _lost = postcard::to_stdvec(&earlier).expect("earlier batch encodes");
        let mut reports = heapless::Vec::new();
        for n in 0..8 {
            reports
                .push(AccelReport {
                    timestamp: report.timestamp.wrapping_sub(n),
                    ..report
                })
                .expect("eight reports fit");
        }
        let packet = crate::Packet {
            data: crate::PacketData::AccelBatch(AccelBatch {
                reports: reports.into(),
            }),
            id: 91,
        };
        let encoded = postcard::to_stdvec(&packet).expect("batch encodes");
        assert!(encoded.len() + 2 <= 247);
        let decoded: crate::Packet =
            postcard::from_bytes(&encoded).expect("batch decodes independently");
        assert_eq!(
            postcard::to_stdvec(&decoded).expect("batch reencodes"),
            encoded
        );
        let crate::PacketData::AccelBatch(decoded) = decoded.data else {
            panic!("batch variant preserved");
        };
        for (n, decoded) in decoded.iter().enumerate() {
            assert_eq!(
                decoded.timestamp,
                u32::MAX.wrapping_sub(u32::try_from(n).expect("eight reports"))
            );
        }
        let mut overfull = postcard::to_stdvec(&9u8).expect("report count encodes");
        for _ in 0..9 {
            overfull.extend(postcard::to_stdvec(&report).expect("report encodes"));
        }
        postcard::from_bytes::<AccelBatch>(&overfull).expect_err("nine reports exceed the bound");
    }
}
