//! Compact forms of the reports a unit streams on its BLE data channel, sent by firmware
//! from device protocol 0.4.0.
//!
//! A data SDU is a postcard [`Packet`], which starts with the index of its `PacketData`
//! variant. Indices 125 and 126 are never given to a variant, so an SDU that starts with
//! one of them is one of these instead:
//!
//! - `125`: a [`MarkersReport`] without its empty slots. A little-endian
//!   `u16` with one bit per occupied slot, `x` and `y` of each occupied slot in slot order
//!   as postcard `u16`s, then the packet id.
//! - `126`: up to [`MAX_BATCH`] [`AccelReport`]s, oldest first. Each is its
//!   timestamp less the previous one's (less zero for the first, wrapping) as a postcard
//!   `u32`, its acceleration and rotation rate as six little-endian `f32`s, then its
//!   packet id.
//!
//! No SDU refers to an earlier one, so a lost SDU costs only the reports it carried.

use heapless::Vec;
use nalgebra::{Point2, Vector3};
use postcard::Error;
use serde::Serialize;

use crate::{AccelReport, MarkersReport, Packet, PacketData};

const SPARSE_MARKERS: u8 = 125;
const ACCEL_BATCH: u8 = 126;

/// Most reports in one batch.
///
/// Since: Unreleased.
pub const MAX_BATCH: usize = 8;

// The unit's data channel has an MPS of 247. A full batch with five-byte timestamps and the
// two-byte SDU length still travels as one PDU.
const _: () = assert!(2 + 1 + MAX_BATCH * (5 + 24 + 1) <= 247);

/// Timestamp difference, acceleration then rotation rate, packet id.
type AccelRecord = (u32, [f32; 6], u8);

/// Writes `packet` as one data SDU and returns the SDU's length. A [`MarkersReport`] with
/// an empty slot takes the sparse form.
///
/// # Errors
/// Returns [`Error::SerializeBufferFull`] if `output` is too short.
///
/// Since: Unreleased.
pub fn encode(packet: &Packet, output: &mut [u8]) -> Result<usize, Error> {
    let PacketData::MarkersReport(report) = &packet.data else {
        return postcard::to_slice(packet, output).map(|sdu| sdu.len());
    };
    let mut occupied = 0u16;
    for (slot, point) in report.points.iter().enumerate() {
        if (point.x, point.y) != (0, 0) {
            occupied |= 1 << slot;
        }
    }
    // An empty slot costs postcard two bytes and the mask costs two, so the sparse form is
    // the longer one only when every slot is occupied.
    if occupied == u16::MAX {
        return postcard::to_slice(packet, output).map(|sdu| sdu.len());
    }
    let mut used = 0;
    put(&(SPARSE_MARKERS, occupied.to_le_bytes()), output, &mut used)?;
    for (slot, point) in report.points.iter().enumerate() {
        if occupied & (1 << slot) != 0 {
            put(&(point.x, point.y), output, &mut used)?;
        }
    }
    put(&packet.id, output, &mut used)?;
    Ok(used)
}

/// Writes `reports`, each with its packet id, as one data SDU and returns the SDU's length.
/// A batch of one is as long as that report's postcard [`Packet`].
///
/// # Errors
/// Returns [`Error::SerializeBufferFull`] for more than [`MAX_BATCH`] reports or an `output`
/// that is too short.
///
/// Since: Unreleased.
pub fn encode_accel_batch(
    reports: &[(u8, AccelReport)],
    output: &mut [u8],
) -> Result<usize, Error> {
    if reports.len() > MAX_BATCH {
        return Err(Error::SerializeBufferFull);
    }
    let mut used = 0;
    put(&ACCEL_BATCH, output, &mut used)?;
    let mut previous = 0;
    for &(id, report) in reports {
        let record: AccelRecord = (
            report.timestamp.wrapping_sub(previous),
            [
                report.accel.x,
                report.accel.y,
                report.accel.z,
                report.gyro.x,
                report.gyro.y,
                report.gyro.z,
            ],
            id,
        );
        put(&record, output, &mut used)?;
        previous = report.timestamp;
    }
    Ok(used)
}

/// Reads the packets of one data SDU, oldest first: one for a postcard [`Packet`] or a
/// sparse marker report, one per report for a batch.
///
/// # Errors
/// Returns the postcard error for an SDU that is none of the three, is cut short, has bytes
/// after a sparse marker report, or batches more than [`MAX_BATCH`] reports. A rejected SDU
/// yields no packet.
///
/// Since: Unreleased.
pub fn decode(input: &[u8]) -> Result<impl Iterator<Item = Packet> + use<>, Error> {
    let mut reports = Vec::<(u8, AccelReport), MAX_BATCH>::new();
    let packet = match input.first() {
        Some(&SPARSE_MARKERS) => Some(sparse_markers(input)?),
        Some(&ACCEL_BATCH) => {
            let mut rest = &input[1..];
            let mut timestamp = 0u32;
            while !rest.is_empty() {
                let ((elapsed, [ax, ay, az, gx, gy, gz], id), tail) =
                    postcard::take_from_bytes::<AccelRecord>(rest)?;
                timestamp = timestamp.wrapping_add(elapsed);
                let report = AccelReport {
                    timestamp,
                    accel: Vector3::new(ax, ay, az),
                    gyro: Vector3::new(gx, gy, gz),
                };
                if reports.push((id, report)).is_err() {
                    return Err(Error::DeserializeBadEncoding);
                }
                rest = tail;
            }
            None
        }
        _ => Some(postcard::from_bytes(input)?),
    };
    Ok(packet
        .into_iter()
        .chain(reports.into_iter().map(|(id, report)| Packet {
            data: PacketData::AccelReport(report),
            id,
        })))
}

fn sparse_markers(input: &[u8]) -> Result<Packet, Error> {
    let ((_, occupied), mut rest) = postcard::take_from_bytes::<(u8, [u8; 2])>(input)?;
    let occupied = u16::from_le_bytes(occupied);
    let mut points = [Point2::new(0, 0); 16];
    for (slot, point) in points.iter_mut().enumerate() {
        if occupied & (1 << slot) != 0 {
            let ((x, y), tail) = postcard::take_from_bytes::<(u16, u16)>(rest)?;
            *point = Point2::new(x, y);
            rest = tail;
        }
    }
    let &[id] = rest else {
        return Err(Error::DeserializeBadEncoding);
    };
    Ok(Packet {
        data: PacketData::MarkersReport(MarkersReport { points }),
        id,
    })
}

fn put(value: &impl Serialize, output: &mut [u8], used: &mut usize) -> Result<(), Error> {
    *used += postcard::to_slice(value, &mut output[*used..])?.len();
    Ok(())
}

#[cfg(test)]
mod tests {
    use nalgebra::{Point2, Vector3};
    use postcard::Error;

    use super::{MAX_BATCH, decode, encode, encode_accel_batch};
    use crate::{AccelReport, ImpactReport, MarkersReport, Packet, PacketData};

    /// Postcard bytes, so that comparing two packets compares their float bit patterns.
    fn wire(packet: &Packet) -> impl PartialEq + use<> {
        postcard::to_stdvec(packet).expect("packet encodes")
    }

    fn decodes_to(sdu: &[u8], want: &[Packet]) -> bool {
        decode(sdu)
            .expect("SDU decodes")
            .map(|packet| wire(&packet))
            .eq(want.iter().map(wire))
    }

    /// Slot 0 has a zero `x` and slot 15 the largest one, so one- to three-byte
    /// coordinates and a half-zero point are all covered.
    fn markers(occupied: u16) -> Packet {
        let points = core::array::from_fn(|slot| {
            if occupied & (1 << slot) == 0 {
                return Point2::new(0, 0);
            }
            let slot = u16::try_from(slot).expect("16 slots");
            Point2::new(slot * 4369, u16::MAX - slot)
        });
        Packet {
            data: PacketData::MarkersReport(MarkersReport { points }),
            id: 234,
        }
    }

    fn accel(timestamp: u32, bits: [u32; 6]) -> AccelReport {
        let [ax, ay, az, gx, gy, gz] = bits.map(f32::from_bits);
        AccelReport {
            timestamp,
            accel: Vector3::new(ax, ay, az),
            gyro: Vector3::new(gx, gy, gz),
        }
    }

    fn accel_packets(reports: &[(u8, AccelReport)]) -> impl Iterator<Item = Packet> {
        reports.iter().map(|&(id, report)| Packet {
            data: PacketData::AccelReport(report),
            id,
        })
    }

    #[test]
    fn markers_round_trip_and_each_empty_slot_saves_two_bytes() {
        for occupied in [0, 1, 0x8202, 0x7fff, u16::MAX] {
            let packet = markers(occupied);
            let mut sdu = [0; 128];
            let len = encode(&packet, &mut sdu).expect("fits");
            let postcard_len = postcard::to_stdvec(&packet).expect("encodes").len();
            let empty = usize::try_from(occupied.count_zeros()).expect("at most 16");
            assert_eq!(
                len,
                (postcard_len + 2 - 2 * empty).min(postcard_len),
                "{occupied:#x}"
            );
            assert!(decodes_to(&sdu[..len], &[packet]), "{occupied:#x}");
        }
    }

    #[test]
    fn other_packets_stay_postcard() {
        let packet = Packet {
            data: PacketData::ImpactReport(ImpactReport { timestamp: 5 }),
            id: 2,
        };
        let mut sdu = [0; 16];
        let len = encode(&packet, &mut sdu).expect("fits");
        assert_eq!(
            &sdu[..len],
            postcard::to_stdvec(&packet).expect("encodes").as_slice()
        );
        assert!(decodes_to(&sdu[..len], &[packet]));
    }

    #[test]
    fn batch_keeps_float_bits_ids_and_timestamps_that_wrap_or_go_back() {
        let reports = [
            (
                17,
                accel(u32::MAX - 1, [0x7fc0_1234, 0x8000_0000, 0, 1, 2, 3]),
            ),
            (241, accel(1, [4, 5, 6, 7, 8, 9])),
            (0, accel(0, [9, 8, 7, 6, 5, 4])),
        ];
        let mut sdu = [0; 256];
        let len = encode_accel_batch(&reports, &mut sdu).expect("fits");
        let mut want = accel_packets(&reports);
        let want = [(); 3].map(|()| want.next().expect("three reports"));
        assert!(decodes_to(&sdu[..len], &want));
    }

    #[test]
    fn batch_saves_the_tag_and_the_long_timestamp_of_every_report_after_the_first() {
        // Five minutes after boot a timestamp takes five bytes; 1 ms apart takes two.
        let reports: [_; MAX_BATCH] = core::array::from_fn(|n| {
            let n = u32::try_from(n).expect("small");
            (3, accel(300_000_000 + 1000 * n, [n; 6]))
        });
        let mut sdu = [0; 256];
        let mut separate = 0;
        for packet in accel_packets(&reports) {
            separate += postcard::to_stdvec(&packet).expect("encodes").len();
        }
        let one = encode_accel_batch(&reports[..1], &mut sdu).expect("fits");
        assert_eq!(one * MAX_BATCH, separate);
        let all = encode_accel_batch(&reports, &mut sdu).expect("fits");
        assert_eq!(separate - all, 4 * (MAX_BATCH - 1));
    }

    #[test]
    fn rejects_cut_short_overlong_and_overfull_compact_sdus() {
        let mut sdu = [0; 256];
        let len = encode(&markers(0x8202), &mut sdu).expect("fits");
        for cut in 1..len {
            assert!(decode(&sdu[..cut]).is_err(), "markers cut to {cut}");
        }
        assert_eq!(
            decode(&sdu[..=len]).err(),
            Some(Error::DeserializeBadEncoding)
        );

        let report = (0, accel(0, [1; 6]));
        let len = encode_accel_batch(&[report; MAX_BATCH], &mut sdu).expect("fits");
        assert_eq!(
            decode(&sdu[..len - 1]).err(),
            Some(Error::DeserializeUnexpectedEnd)
        );
        assert_eq!(
            encode_accel_batch(&[report; MAX_BATCH + 1], &mut sdu),
            Err(Error::SerializeBufferFull)
        );
        let record = (len - 1) / MAX_BATCH;
        sdu.copy_within(1..=record, len);
        assert_eq!(
            decode(&sdu[..len + record]).err(),
            Some(Error::DeserializeBadEncoding)
        );
    }

    /// Fails when `PacketData` grows a variant whose index one of the compact forms uses.
    #[test]
    fn compact_tags_are_not_packets() {
        for tag in [125, 126] {
            assert!(postcard::from_bytes::<Packet>(&[tag; 64]).is_err(), "{tag}");
        }
    }
}
