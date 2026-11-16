//! The packed binary packet format that preceded postcard and CBOR.

use core::mem::MaybeUninit;
#[cfg(feature = "std")]
use std::vec::Vec;

use nalgebra::Point2;

use super::{CombinedMarkersReport, MotData, ObjectReport};
use crate::{Error, Packet, PacketType};

impl Packet {
    /// Writes the packet header followed by `data` and returns the bytes
    /// written.
    ///
    /// The header is a little-endian length in 16-bit words (header
    /// included), the packet type and the request id.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn serialize_parts<D: Serialize>(
        id: u8,
        ty: PacketType,
        data: &D,
        mut buf: &mut [MaybeUninit<u8>],
    ) -> usize {
        const HEADER_SIZE: usize = 4;
        let orig_buf_len = buf.len();
        let [words_low, words_high] =
            u16::to_le_bytes(usize::midpoint(D::SIZE, HEADER_SIZE) as u16);
        push(&mut buf, &[words_low, words_high, ty.into(), id]);
        data.serialize(&mut buf);
        orig_buf_len - buf.len()
    }
}

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
///
/// Since: Odyssey Desktop 0.6.0.
pub trait Parse: Sized {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// # Errors
    /// Returns an error if the input is truncated or contains invalid packet data.
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error>;
}

/// Writes a value into an uninitialised buffer, advancing the buffer past it.
///
/// # Safety
/// Implementations must initialise exactly the first [`Self::SIZE`] bytes of
/// the original buffer and advance the cursor past those bytes. Safe callers
/// may expose those bytes as initialised memory after this method returns.
///
/// # Since
/// Odyssey Desktop 0.6.0.
#[expect(
    unsafe_code,
    reason = "implementors guarantee initialisation of the output"
)]
pub unsafe trait Serialize {
    /// Exactly the number of bytes [`serialize`](Self::serialize) writes.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    const SIZE: usize;

    /// Writes [`SIZE`](Self::SIZE) bytes to the front of `buf` and advances it.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    #[expect(
        clippy::mut_mut,
        reason = "serialization advances the caller's slice cursor"
    )]
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]);
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg(feature = "std")]
    #[expect(
        unsafe_code,
        reason = "set_len over the spare capacity that serialize has filled"
    )]
    fn serialize_to_vec(&self, buf: &mut Vec<u8>) {
        buf.reserve(Self::SIZE);
        let spare = buf.spare_capacity_mut();
        let spare_len = spare.len();
        let mut cursor = spare;
        self.serialize(&mut cursor);
        let written = spare_len - cursor.len();
        // The unsafe trait contract guarantees the first `SIZE` bytes were
        // initialised. Checking the cursor also catches size mistakes.
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
#[expect(clippy::mut_mut, reason = "push advances the caller's slice cursor")]
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

#[expect(unsafe_code, reason = "the serializer initialises exactly SIZE bytes")]
#[expect(
    clippy::mut_mut,
    reason = "serialization advances the caller's slice cursor"
)]
// SAFETY: `push` initialises and advances past all 16 bytes.
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
                packet_type: Some(PacketType::LegacyObjectReport()),
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

#[expect(unsafe_code, reason = "the serializer initialises exactly SIZE bytes")]
#[expect(
    clippy::mut_mut,
    reason = "serialization advances the caller's slice cursor"
)]
// SAFETY: each field serializer and `push` initialises its entire wire field.
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
                packet_type: Some(PacketType::LegacyCombinedMarkersReport()),
            });
        }
        let (report, rest) = bytes.split_at(Self::SIZE);
        *bytes = rest;

        let (packed_points, _) = report.as_chunks::<PACKED_POINT_SIZE>();
        let mut points = [Point2::new(0, 0); 2 * MOT_DATA_PER_CAMERA];
        for (point, packed) in points.iter_mut().zip(packed_points) {
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

#[expect(unsafe_code, reason = "the serializer initialises exactly SIZE bytes")]
#[expect(
    clippy::mut_mut,
    reason = "serialization advances the caller's slice cursor"
)]
// SAFETY: `push` initialises three bytes for each of the 32 points.
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

#[cfg(all(kani, feature = "std"))]
mod kani_proofs {
    use core::mem::MaybeUninit;
    use std::vec::Vec;

    use super::{Parse, Serialize};
    use crate::legacy::{CombinedMarkersReport, MotData, ObjectReport};
    use crate::{Error, Packet, PacketType};

    #[kani::proof]
    #[kani::unwind(18)]
    fn mot_data_offsets_masks_and_canonical_bytes() {
        let raw: [u8; 16] = kani::any();
        let mut input = &raw[..];
        let Ok(data) = MotData::parse(&mut input) else {
            panic!("full object record rejected");
        };
        assert!(input.is_empty());
        assert_eq!(data.area, u16::from_le_bytes([raw[0], raw[1]]));
        assert_eq!(data.cx, u16::from_le_bytes([raw[2], raw[3] & 0x0f]));
        assert_eq!(data.cy, u16::from_le_bytes([raw[4], raw[5] & 0x0f]));
        assert_eq!(data.avg_brightness, raw[6]);
        assert_eq!(data.max_brightness, raw[7]);
        assert_eq!(data.range, raw[8] >> 4);
        assert_eq!(data.radius, raw[8] & 0x0f);
        assert_eq!(data.boundary_left, raw[9] & 0x7f);
        assert_eq!(data.boundary_right, raw[10] & 0x7f);
        assert_eq!(data.boundary_up, raw[11] & 0x7f);
        assert_eq!(data.boundary_down, raw[12] & 0x7f);
        assert_eq!(data.aspect_ratio, raw[13]);
        assert_eq!(data.vx, raw[14]);
        assert_eq!(data.vy, raw[15]);
        let mut encoded = Vec::new();
        data.serialize_to_vec(&mut encoded);
        assert_eq!(
            encoded.as_slice(),
            &[
                raw[0],
                raw[1],
                raw[2],
                raw[3] & 0x0f,
                raw[4],
                raw[5] & 0x0f,
                raw[6],
                raw[7],
                raw[8],
                raw[9] & 0x7f,
                raw[10] & 0x7f,
                raw[11] & 0x7f,
                raw[12] & 0x7f,
                raw[13],
                raw[14],
                raw[15],
            ]
        );
    }

    #[kani::proof]
    #[kani::unwind(2)]
    fn mot_data_short_input_preserves_cursor() {
        let raw: [u8; 16] = kani::any();
        let len: usize = kani::any();
        kani::assume(len < 16);
        let mut input = &raw[..len];
        let before = input;
        assert!(matches!(
            MotData::parse(&mut input),
            Err(Error::UnexpectedEof { packet_type: None })
        ));
        assert_eq!(input.len(), before.len());
        assert_eq!(input.as_ptr(), before.as_ptr());
    }

    #[kani::proof]
    #[kani::unwind(34)]
    #[expect(
        unsafe_code,
        reason = "the proof buffer is initialized before serialization"
    )]
    fn combined_markers_uses_frozen_12_bit_layout_and_roundtrips() {
        let raw: [u8; 96] = kani::any();
        let mut input = &raw[..];
        let Ok(report) = CombinedMarkersReport::parse(&mut input) else {
            panic!("full marker report rejected");
        };
        assert!(input.is_empty());
        for i in 0..32 {
            let offset = i * 3;
            let point = if i < 16 {
                report.nf_points[i]
            } else {
                report.wf_points[i - 16]
            };
            let x = u16::from(raw[offset]) | (u16::from(raw[offset + 1] & 0x0f) << 8);
            let y = u16::from(raw[offset + 1] >> 4) | (u16::from(raw[offset + 2]) << 4);
            assert_eq!(point.x, x);
            assert_eq!(point.y, y);
        }
        let mut encoded = [MaybeUninit::new(0xa5); 96];
        let mut cursor = &mut encoded[..];
        report.serialize(&mut cursor);
        assert!(cursor.is_empty());
        let byte: usize = kani::any();
        kani::assume(byte < 96);
        // SAFETY: every buffer slot starts initialized; serialization only writes u8 values.
        let actual = unsafe { encoded[byte].assume_init() };
        assert_eq!(actual, raw[byte]);
    }

    #[kani::proof]
    #[kani::unwind(34)]
    #[expect(
        unsafe_code,
        reason = "the proof buffer is initialized before serialization"
    )]
    fn object_report_has_frozen_offsets_and_trailer() {
        let raw: [u8; 518] = kani::any();
        let mut input = &raw[..];
        let Ok(report) = ObjectReport::parse(&mut input) else {
            panic!("full object report rejected");
        };
        assert!(input.is_empty());
        assert_eq!(
            report.timestamp,
            u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
        );
        for i in 0..32 {
            let offset = 4 + i * 16;
            let item = if i < 16 {
                report.mot_data_nf[i]
            } else {
                report.mot_data_wf[i - 16]
            };
            assert_eq!(
                item.area,
                u16::from_le_bytes([raw[offset], raw[offset + 1]])
            );
            assert_eq!(item.vy, raw[offset + 15]);
        }
        let mut encoded = [MaybeUninit::new(0xa5); 518];
        let mut cursor = &mut encoded[..];
        report.serialize(&mut cursor);
        assert!(cursor.is_empty());
        let byte: usize = kani::any();
        kani::assume(byte < 518);
        let expected = match byte {
            516 => 1,
            517 => 0,
            0..=3 => raw[byte],
            _ => match (byte - 4) % 16 {
                3 | 5 => raw[byte] & 0x0f,
                9..=12 => raw[byte] & 0x7f,
                _ => raw[byte],
            },
        };
        // SAFETY: every buffer slot starts initialized; serialization only writes u8 values.
        let actual = unsafe { encoded[byte].assume_init() };
        assert_eq!(actual, expected);
    }

    #[kani::proof]
    #[kani::unwind(2)]
    fn object_report_short_input_preserves_cursor() {
        let raw: [u8; 518] = kani::any();
        let len: usize = kani::any();
        kani::assume(len < 518);
        let mut input = &raw[..len];
        let before = input;
        assert!(matches!(
            ObjectReport::parse(&mut input),
            Err(Error::UnexpectedEof {
                packet_type: Some(PacketType::LegacyObjectReport())
            })
        ));
        assert_eq!(input.len(), before.len());
        assert_eq!(input.as_ptr(), before.as_ptr());
    }

    #[kani::proof]
    #[kani::unwind(2)]
    fn combined_markers_short_input_preserves_cursor() {
        let raw: [u8; 96] = kani::any();
        let len: usize = kani::any();
        kani::assume(len < 96);
        let mut input = &raw[..len];
        let before = input;
        assert!(matches!(
            CombinedMarkersReport::parse(&mut input),
            Err(Error::UnexpectedEof {
                packet_type: Some(PacketType::LegacyCombinedMarkersReport())
            })
        ));
        assert_eq!(input.len(), before.len());
        assert_eq!(input.as_ptr(), before.as_ptr());
    }

    #[kani::proof]
    #[kani::unwind(22)]
    #[expect(
        unsafe_code,
        reason = "Packet::serialize_parts writes the full fixed header and payload"
    )]
    fn packet_header_uses_frozen_length_type_and_id_bytes() {
        let id: u8 = kani::any();
        let data = MotData::default();
        let mut storage = [MaybeUninit::uninit(); 20];
        let written = Packet::serialize_parts(
            id,
            PacketType::LegacyObjectReport(),
            &data,
            &mut storage[..],
        );
        assert_eq!(written, 20);
        let mut bytes = [0; 20];
        for (destination, source) in bytes.iter_mut().zip(storage) {
            // SAFETY: Serialize guarantees initialization of its entire fixed payload.
            *destination = unsafe { source.assume_init() };
        }
        assert_eq!(&bytes[..4], &[10, 0, 9, id]);
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
        assert_eq!(cursor, [0u8; 0]);
    }
}
