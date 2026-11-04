use core::mem::MaybeUninit;
#[cfg(feature = "std")]
use std::vec::Vec;

use nalgebra::Point2;

use crate::{CombinedMarkersReport, Error, MotData, ObjectReport, PacketType};

/// Bytes of one [`MotData`] on the wire.
const MOT_DATA_SIZE: usize = 16;
/// Each camera (near field, wide field) reports this many objects.
const MOT_DATA_PER_CAMERA: usize = 16;
/// Bytes of the timestamp that opens an [`ObjectReport`].
const TIMESTAMP_SIZE: usize = size_of::<u32>();
/// Trailer of an [`ObjectReport`]: a format tag whose only defined value is 1,
/// then a reserved byte. Parsing ignores it.
const OBJECT_REPORT_TRAILER: [u8; 2] = [1, 0];
/// A marker position is two 12-bit coordinates packed into 3 bytes.
const PACKED_POINT_SIZE: usize = 3;
/// Mask of the 12-bit coordinate's high nibble in its middle byte.
const HIGH_NIBBLE_MASK: u8 = 0x0f;

/// Reads a value off the front of `bytes`, advancing the slice past it.
pub trait Parse: Sized {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error>;
}

/// Writes a value into an uninitialised buffer, advancing the buffer past it.
///
/// # Safety
/// Implementations must initialise exactly the first [`Self::SIZE`] bytes of
/// the original buffer and advance the cursor past those bytes. Safe callers
/// may expose those bytes as initialised memory after this method returns.
#[expect(
    unsafe_code,
    reason = "implementors guarantee initialisation of the output"
)]
pub unsafe trait Serialize {
    /// Exactly the number of bytes [`serialize`](Self::serialize) writes.
    const SIZE: usize;

    /// Writes [`SIZE`](Self::SIZE) bytes to the front of `buf` and advances it.
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]);

    #[cfg(feature = "std")]
    #[expect(
        unsafe_code,
        reason = "set_len over the spare capacity that serialize just filled"
    )]
    fn serialize_to_vec(&self, buf: &mut Vec<u8>) {
        buf.reserve(Self::SIZE);
        let spare = buf.spare_capacity_mut();
        let spare_len = spare.len();
        let mut cursor = spare;
        self.serialize(&mut cursor);
        let written = spare_len - cursor.len();
        // The unsafe trait contract guarantees that the first `SIZE` bytes
        // were initialised; checking the cursor also catches size mistakes.
        assert_eq!(written, Self::SIZE, "serialize wrote a different SIZE");
        // SAFETY: `Serialize` guarantees initialisation of the first `SIZE`
        // bytes, and the cursor check confirms the promised length.
        unsafe {
            buf.set_len(buf.len() + written);
        }
    }
}

/// Copies `data` to the front of `buf` and advances `buf` past it.
///
/// Panics if `buf` is shorter than `data`: `Serialize::SIZE` promises the room.
#[inline]
pub(crate) fn push(buf: &mut &mut [MaybeUninit<u8>], data: &[u8]) {
    let (head, tail) = core::mem::take(buf).split_at_mut(data.len());
    for (slot, byte) in head.iter_mut().zip(data) {
        slot.write(*byte);
    }
    *buf = tail;
}

impl Parse for MotData {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        let Some((raw, rest)) = bytes.split_first_chunk::<MOT_DATA_SIZE>() else {
            return Err(Error::UnexpectedEof { packet_type: None });
        };
        *bytes = rest;
        Ok(Self {
            area: u16::from_le_bytes([raw[0], raw[1]]),
            cx: u16::from_le_bytes([raw[2], raw[3] & HIGH_NIBBLE_MASK]),
            cy: u16::from_le_bytes([raw[4], raw[5] & HIGH_NIBBLE_MASK]),
            avg_brightness: raw[6],
            max_brightness: raw[7],
            radius: raw[8] & 0x0f,
            range: raw[8] >> 4,
            boundary_left: raw[9] & 0x7f,
            boundary_right: raw[10] & 0x7f,
            boundary_up: raw[11] & 0x7f,
            boundary_down: raw[12] & 0x7f,
            aspect_ratio: raw[13],
            vx: raw[14],
            vy: raw[15],
        })
    }
}

// SAFETY: `push` initialises and advances past all 16 bytes.
#[expect(unsafe_code, reason = "the serializer initialises exactly SIZE bytes")]
unsafe impl Serialize for MotData {
    const SIZE: usize = MOT_DATA_SIZE;

    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        let [area_low, area_high] = self.area.to_le_bytes();
        let [cx_low, cx_high] = self.cx.to_le_bytes();
        let [cy_low, cy_high] = self.cy.to_le_bytes();
        push(
            buf,
            &[
                area_low,
                area_high,
                cx_low,
                cx_high,
                cy_low,
                cy_high,
                self.avg_brightness,
                self.max_brightness,
                self.radius | self.range << 4,
                self.boundary_left,
                self.boundary_right,
                self.boundary_up,
                self.boundary_down,
                self.aspect_ratio,
                self.vx,
                self.vy,
            ],
        );
    }
}

/// Parses `MOT_DATA_PER_CAMERA` consecutive records.
fn parse_mot_data_array(bytes: &mut &[u8]) -> Result<[MotData; MOT_DATA_PER_CAMERA], Error> {
    let mut objects = [MotData::default(); MOT_DATA_PER_CAMERA];
    for object in &mut objects {
        *object = MotData::parse(bytes)?;
    }
    Ok(objects)
}

impl Parse for ObjectReport {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        if bytes.len() < Self::SIZE {
            return Err(Error::UnexpectedEof {
                packet_type: Some(PacketType::ObjectReport()),
            });
        }
        let (report, rest) = bytes.split_at(Self::SIZE);
        *bytes = rest;

        let (timestamp, mut objects) = report.split_at(TIMESTAMP_SIZE);
        Ok(Self {
            timestamp: u32::from_le_bytes([timestamp[0], timestamp[1], timestamp[2], timestamp[3]]),
            mot_data_nf: parse_mot_data_array(&mut objects)?,
            mot_data_wf: parse_mot_data_array(&mut objects)?,
        })
    }
}

// SAFETY: each field serializer and `push` initialises its entire wire field.
#[expect(unsafe_code, reason = "the serializer initialises exactly SIZE bytes")]
unsafe impl Serialize for ObjectReport {
    const SIZE: usize =
        TIMESTAMP_SIZE + 2 * MOT_DATA_PER_CAMERA * MOT_DATA_SIZE + OBJECT_REPORT_TRAILER.len();

    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        push(buf, &self.timestamp.to_le_bytes());
        for object in self.mot_data_nf.iter().chain(&self.mot_data_wf) {
            object.serialize(buf);
        }
        push(buf, &OBJECT_REPORT_TRAILER);
    }
}

impl Parse for CombinedMarkersReport {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        if bytes.len() < Self::SIZE {
            return Err(Error::UnexpectedEof {
                packet_type: Some(PacketType::CombinedMarkersReport()),
            });
        }
        let (report, rest) = bytes.split_at(Self::SIZE);
        *bytes = rest;

        let (packed_points, _) = report.as_chunks::<PACKED_POINT_SIZE>();
        let mut points = [Point2::new(0, 0); 2 * MOT_DATA_PER_CAMERA];
        for (point, packed) in points.iter_mut().zip(packed_points) {
            // x, y is 12 bits each
            let x = u16::from_le_bytes([packed[0], packed[1] & HIGH_NIBBLE_MASK]);
            let y = u16::from(packed[1] >> 4) | (u16::from(packed[2]) << 4);
            *point = Point2::new(x, y);
        }

        Ok(Self {
            nf_points: core::array::from_fn(|i| points[i]),
            wf_points: core::array::from_fn(|i| points[MOT_DATA_PER_CAMERA + i]),
        })
    }
}

// SAFETY: `push` initialises three bytes for each of the 32 points.
#[expect(unsafe_code, reason = "the serializer initialises exactly SIZE bytes")]
unsafe impl Serialize for CombinedMarkersReport {
    const SIZE: usize = 2 * MOT_DATA_PER_CAMERA * PACKED_POINT_SIZE;

    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        for p in self.nf_points.iter().chain(&self.wf_points) {
            let (x, y) = (p.x, p.y);
            let byte0 = x & 0xff;
            let byte1 = ((x >> 8) & 0x0f) | ((y & 0x0f) << 4);
            let byte2 = y >> 4;
            push(buf, &[byte0 as u8, byte1 as u8, byte2 as u8]);
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    fn sample_mot_data(seed: u8) -> MotData {
        MotData {
            area: 0x0100 | u16::from(seed),
            cx: 0x0a00 | u16::from(seed),
            cy: 0x0500 | u16::from(seed),
            avg_brightness: seed,
            max_brightness: seed.wrapping_add(1),
            range: seed & 0x0f,
            radius: (seed >> 4) & 0x0f,
            boundary_left: seed & 0x7f,
            boundary_right: (seed + 1) & 0x7f,
            boundary_up: (seed + 2) & 0x7f,
            boundary_down: (seed + 3) & 0x7f,
            aspect_ratio: seed,
            vx: seed,
            vy: seed,
        }
    }

    fn serialized<T: Serialize>(value: &T) -> Vec<u8> {
        let mut bytes = Vec::new();
        value.serialize_to_vec(&mut bytes);
        bytes
    }

    #[test]
    fn mot_data_layout_is_pinned() {
        let mot_data = MotData {
            area: 0x0102,
            cx: 0x0a03,
            cy: 0x0504,
            avg_brightness: 0x06,
            max_brightness: 0x07,
            radius: 0x3,
            range: 0x2,
            boundary_left: 0x10,
            boundary_right: 0x11,
            boundary_up: 0x12,
            boundary_down: 0x13,
            aspect_ratio: 0x14,
            vx: 0x15,
            vy: 0x16,
        };

        let bytes = serialized(&mot_data);

        assert_eq!(
            bytes,
            [
                0x02, 0x01, 0x03, 0x0a, 0x04, 0x05, 0x06, 0x07, 0x23, 0x10, 0x11, 0x12, 0x13, 0x14,
                0x15, 0x16
            ]
        );
        assert_eq!(MotData::parse(&mut bytes.as_slice()).unwrap(), mot_data);
    }

    #[test]
    fn object_report_round_trips_every_slot_in_place() {
        let report = ObjectReport {
            timestamp: 0xdead_beef,
            mot_data_nf: core::array::from_fn(|i| sample_mot_data(i as u8)),
            mot_data_wf: core::array::from_fn(|i| sample_mot_data(0x40 + i as u8)),
        };

        let mut bytes = serialized(&report);
        assert_eq!(bytes.len(), ObjectReport::SIZE);
        assert_eq!(bytes[..4], 0xdead_beef_u32.to_le_bytes());
        assert_eq!(bytes[bytes.len() - 2..], [1, 0]);

        bytes.push(0x55);
        let mut cursor = bytes.as_slice();
        assert_eq!(ObjectReport::parse(&mut cursor).unwrap(), report);
        assert_eq!(cursor, [0x55]);
    }

    #[test]
    fn combined_markers_round_trip() {
        let report = CombinedMarkersReport {
            nf_points: core::array::from_fn(|i| Point2::new(i as u16 * 100, 4095 - i as u16)),
            wf_points: core::array::from_fn(|i| Point2::new(4095 - i as u16, i as u16 * 7)),
        };

        let bytes = serialized(&report);

        assert_eq!(bytes.len(), CombinedMarkersReport::SIZE);
        assert_eq!(
            CombinedMarkersReport::parse(&mut bytes.as_slice()).unwrap(),
            report
        );
    }

    #[test]
    fn every_truncated_prefix_is_rejected_without_consuming_input() {
        fn check<T: Parse + Serialize>(value: &T) {
            let bytes = serialized(value);
            for length in 0..T::SIZE {
                let mut cursor = &bytes[..length];
                assert!(matches!(
                    T::parse(&mut cursor),
                    Err(Error::UnexpectedEof { .. })
                ));
                assert_eq!(cursor, &bytes[..length]);
            }
        }
        check(&MotData::default());
        check(&ObjectReport::default());
        check(&CombinedMarkersReport::default());
    }

    #[test]
    fn serializing_appends_without_overwriting_existing_bytes() {
        let first = sample_mot_data(1);
        let second = sample_mot_data(2);
        let mut bytes = Vec::from([0xaa, 0xbb]);
        first.serialize_to_vec(&mut bytes);
        second.serialize_to_vec(&mut bytes);
        assert_eq!(&bytes[..2], &[0xaa, 0xbb]);
        let mut cursor = &bytes[2..];
        assert_eq!(MotData::parse(&mut cursor).unwrap(), first);
        assert_eq!(MotData::parse(&mut cursor).unwrap(), second);
        assert!(cursor.is_empty());
    }
}
