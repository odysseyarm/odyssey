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

/// Marker positions without the empty slots of a [`MarkersReport`].
///
/// Bit `i` of `mask` is set when slot `i` holds a marker. `points` lists the occupied
/// slots in ascending slot order, so its length equals the number of set bits.
///
/// The [crate documentation](crate) links the report streaming design.
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

/// A [`SparseMarkersReport`] whose point count differs from the number of set mask bits.
/// Its points cannot be assigned to slots, so the report is discarded.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SparseMarkersError;

impl core::fmt::Display for SparseMarkersError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("marker occupancy mask and point count disagree")
    }
}

impl core::error::Error for SparseMarkersError {}

impl From<MarkersReport> for SparseMarkersReport {
    fn from(report: MarkersReport) -> Self {
        let occupied = |point: &Point2<u16>| *point != Point2::origin();
        let mut mask = 0;
        for (slot, point) in report.points.iter().enumerate() {
            if occupied(point) {
                mask |= 1 << slot;
            }
        }
        let points: heapless::Vec<_, 16> = report.points.into_iter().filter(occupied).collect();
        Self {
            mask,
            points: points.into(),
        }
    }
}

impl From<MarkersReport> for crate::PacketData {
    /// Picks the marker form with the shorter postcard encoding: the dense form when
    /// at most one slot is empty, the sparse form otherwise.
    fn from(report: MarkersReport) -> Self {
        let sparse = SparseMarkersReport::from(report);
        // Sparse spends a mask of up to three bytes and a count byte; dense spends two
        // bytes on each empty slot.
        if sparse.points.len() <= 14 {
            Self::SparseMarkersReport(sparse)
        } else {
            Self::MarkersReport(report)
        }
    }
}

impl TryFrom<SparseMarkersReport> for MarkersReport {
    type Error = SparseMarkersError;

    /// Returns each point to its slot and leaves the other slots at the origin.
    fn try_from(sparse: SparseMarkersReport) -> Result<Self, Self::Error> {
        if sparse.mask.count_ones() as usize != sparse.points.len() {
            return Err(SparseMarkersError);
        }
        let mut points = [Point2::origin(); 16];
        let occupied_slots = (0..16).filter(|slot| sparse.mask & (1 << slot) != 0);
        for (slot, point) in occupied_slots.zip(sparse.points.iter()) {
            points[slot] = *point;
        }
        Ok(Self { points })
    }
}

/// IMU reports of one stream in measurement order.
///
/// Each report keeps its own timestamp and values. A batch holds at most eight reports,
/// which keeps a full batch within one PDU of the BLE data channel.
///
/// The [crate documentation](crate) links the report streaming design.
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

/// Motion since the previous report: mean acceleration (m/s²) and mean rotation rate (rad/s)
/// over the interval ending at `timestamp` (µs, wrapping).
///
/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AccelReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
    #[cfg_attr(feature = "schema", postcard_type = "[f32; 3]")]
    pub accel: Vector3<f32>,
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

    use super::{AccelReport, MarkersReport, SparseMarkersError, SparseMarkersReport};
    use crate::{List, Packet, PacketData};

    fn accel_report(timestamp: u32) -> AccelReport {
        AccelReport {
            timestamp,
            accel: Vector3::new(-0.0, f32::from_bits(0x7fc0_1234), 1.0),
            gyro: Vector3::new(2.0, 3.0, 4.0),
        }
    }

    fn accel_packet(id: u8, timestamp: u32) -> Packet {
        Packet {
            id,
            data: PacketData::AccelReport(accel_report(timestamp)),
        }
    }

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
        let sparse = SparseMarkersReport::from(report);
        assert_eq!(sparse.mask, (1 << 2) | (1 << 14));
        assert_eq!(sparse.points.len(), 2);
        assert_eq!(MarkersReport::try_from(sparse), Ok(report));
        assert_eq!(
            MarkersReport::try_from(SparseMarkersReport {
                mask: 1,
                points: List::default()
            }),
            Err(SparseMarkersError)
        );
        let unmasked = heapless::Vec::from_slice(&[Point2::new(1, 2)]).expect("fits");
        assert_eq!(
            MarkersReport::try_from(SparseMarkersReport {
                mask: 0,
                points: unmasked.into()
            }),
            Err(SparseMarkersError)
        );
    }

    #[test]
    fn a_full_marker_report_survives_the_sparse_form() {
        let report = MarkersReport {
            points: core::array::from_fn(|slot| Point2::new(slot as u16 + 1, u16::MAX)),
        };
        let sparse = SparseMarkersReport::from(report);
        assert_eq!(sparse.mask, u16::MAX);
        assert_eq!(MarkersReport::try_from(sparse), Ok(report));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn the_chosen_marker_form_is_never_longer_than_the_dense_form() {
        for occupied in 0..=16 {
            // The highest slots give the longest mask.
            let report = MarkersReport {
                points: core::array::from_fn(|slot| {
                    if slot >= 16 - occupied {
                        Point2::new(u16::MAX, u16::MAX)
                    } else {
                        Point2::origin()
                    }
                }),
            };
            let dense = postcard::to_stdvec(&PacketData::MarkersReport(report)).expect("encodes");
            let chosen = PacketData::from(report);
            let encoded = postcard::to_stdvec(&chosen).expect("encodes");
            assert!(encoded.len() <= dense.len(), "{occupied} occupied");
            assert_eq!(
                matches!(chosen, PacketData::SparseMarkersReport(_)),
                occupied <= 14,
                "{occupied} occupied"
            );
        }
    }

    #[test]
    fn queued_accel_reports_join_in_order_up_to_one_full_batch() {
        let mut joined = accel_packet(91, 0);
        for timestamp in 1..8 {
            joined = joined
                .join_accel(&accel_packet(91, timestamp))
                .expect("eight reports share a batch");
        }
        let PacketData::AccelBatch(batch) = &joined.data else {
            panic!("joined reports form a batch");
        };
        assert_eq!(joined.id, 91);
        let timestamps: heapless::Vec<u32, 8> = batch
            .reports
            .iter()
            .map(|report| report.timestamp)
            .collect();
        assert_eq!(timestamps, [0, 1, 2, 3, 4, 5, 6, 7]);

        assert!(joined.join_accel(&accel_packet(91, 8)).is_none(), "ninth");
        assert!(
            accel_packet(91, 0)
                .join_accel(&accel_packet(92, 1))
                .is_none(),
            "another stream id"
        );
        let ack = Packet {
            id: 91,
            data: PacketData::Ack(),
        };
        assert!(accel_packet(91, 0).join_accel(&ack).is_none(), "not IMU");
        assert!(ack.join_accel(&accel_packet(91, 0)).is_none(), "not IMU");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn postcard_roundtrip_preserves_report_float_bits() {
        let report = AccelReport {
            timestamp: 0xdead_beef,
            accel: Vector3::new(f32::from_bits(0x7fc0_1234), -0.0, f32::from_bits(1)),
            gyro: Vector3::new(f32::from_bits(0xffc0_5678), 3.0, 4.0),
        };
        let batch = super::AccelBatch {
            reports: heapless::Vec::from_slice(&[report])
                .expect("one report fits")
                .into(),
        };
        let packet = Packet {
            data: PacketData::AccelBatch(batch),
            id: 91,
        };
        let bytes = postcard::to_stdvec(&packet).expect("serialize packet");
        let decoded: Packet = postcard::from_bytes(&bytes).expect("deserialize packet");
        let PacketData::AccelBatch(decoded) = decoded.data else {
            panic!("batch variant preserved");
        };
        let report = decoded.reports.first().expect("batch has report");
        assert_eq!(report.timestamp, 0xdead_beef);
        assert_eq!(report.accel.x.to_bits(), 0x7fc0_1234);
        assert_eq!(report.accel.y.to_bits(), 0x8000_0000);
        assert_eq!(report.accel.z.to_bits(), 1);
        assert_eq!(report.gyro.x.to_bits(), 0xffc0_5678);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn full_batch_fits_one_pdu_and_a_ninth_report_is_rejected() {
        let report = accel_report(u32::MAX);
        let mut reports = heapless::Vec::new();
        for n in 0..8 {
            reports
                .push(AccelReport {
                    timestamp: report.timestamp.wrapping_sub(n),
                    ..report
                })
                .expect("eight reports fit");
        }
        let packet = Packet {
            data: PacketData::AccelBatch(super::AccelBatch {
                reports: reports.into(),
            }),
            id: 91,
        };
        let encoded = postcard::to_stdvec(&packet).expect("batch encodes");
        // Timestamps above 2^28 take the longest varint, so this is the largest batch packet.
        assert_eq!(
            encoded.len(),
            235,
            "must stay within the 245-byte BLE packing target"
        );
        let decoded: Packet = postcard::from_bytes(&encoded).expect("batch decodes independently");
        assert_eq!(
            postcard::to_stdvec(&decoded).expect("batch reencodes"),
            encoded
        );
        let PacketData::AccelBatch(decoded) = decoded.data else {
            panic!("batch variant preserved");
        };
        for (n, decoded) in decoded.reports.iter().enumerate() {
            assert_eq!(
                decoded.timestamp,
                u32::MAX.wrapping_sub(u32::try_from(n).expect("eight reports"))
            );
        }
        let mut overfull = postcard::to_stdvec(&9u8).expect("report count encodes");
        for _ in 0..9 {
            overfull.extend(postcard::to_stdvec(&report).expect("report encodes"));
        }
        postcard::from_bytes::<super::AccelBatch>(&overfull)
            .expect_err("nine reports exceed the bound");
    }
}
