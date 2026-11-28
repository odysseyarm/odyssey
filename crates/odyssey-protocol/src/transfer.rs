use crate::Packet;

/// Encodes `first` into `buf`, then the packets `queued` yields for as long as the
/// transfer stays within `target` bytes. One transfer is one USB bulk transfer or
/// one BLE SDU. `first` alone may exceed `target`.
///
/// IMU reports that follow each other under one id share an `AccelBatch`
/// ([`Packet::join_accel`]). No packet moves past another, so a receiver that
/// decodes the transfer front to back sees the queue order.
///
/// `encode` writes one packet into the slice and returns the bytes written. It
/// fails when the packet does not fit the slice.
///
/// Returns the bytes used and the packet that did not fit. That packet starts the
/// next transfer.
///
/// # Errors
///
/// Returns the error of `encode` when `first` does not fit `buf`. No queued packet
/// has been taken in that case.
///
/// Since: Unreleased.
pub fn pack_queued<E>(
    first: Packet,
    mut queued: impl FnMut() -> Option<Packet>,
    mut encode: impl FnMut(&Packet, &mut [u8]) -> Result<usize, E>,
    buf: &mut [u8],
    target: usize,
) -> Result<(usize, Option<Packet>), E> {
    let target = target.min(buf.len());
    let mut last = first;
    let mut last_start = 0;
    let mut used = encode(&last, buf)?;
    while used < target {
        let Some(next) = queued() else { break };
        if let Some(joined) = last.join_accel(&next) {
            let Ok(length) = encode(&joined, &mut buf[last_start..target]) else {
                // The failed attempt wrote over the bytes of `last`.
                encode(&last, &mut buf[last_start..])?;
                return Ok((used, Some(next)));
            };
            used = last_start + length;
            last = joined;
            continue;
        }
        let Ok(length) = encode(&next, &mut buf[used..target]) else {
            return Ok((used, Some(next)));
        };
        last_start = used;
        used += length;
        last = next;
    }
    Ok((used, None))
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use nalgebra::{Point2, Vector3};

    use super::pack_queued;
    use crate::{AccelReport, MarkersReport, Packet, PacketData};

    /// The packing target of the unit firmware: one BLE data PDU less the SDU length.
    const TARGET: usize = 245;

    /// A timestamp above 2^28 takes the longest varint, which makes the packet 31 bytes.
    fn accel(id: u8, n: u32) -> Packet {
        Packet {
            id,
            data: PacketData::AccelReport(AccelReport {
                timestamp: u32::MAX - n,
                accel: Vector3::new(1.0, 2.0, 3.0),
                gyro: Vector3::new(4.0, 5.0, 6.0),
            }),
        }
    }

    /// Sixteen occupied slots with three-byte coordinates: 98 bytes.
    fn markers(id: u8) -> Packet {
        Packet {
            id,
            data: PacketData::MarkersReport(MarkersReport {
                points: [Point2::new(u16::MAX, u16::MAX); 16],
            }),
        }
    }

    fn encode(packet: &Packet, out: &mut [u8]) -> Result<usize, postcard::Error> {
        postcard::to_slice(packet, out).map(|encoded| encoded.len())
    }

    fn pack<const N: usize>(packets: [Packet; N]) -> ([u8; 1024], usize, Option<Packet>, usize) {
        let mut queue = heapless::Deque::<Packet, 16>::new();
        for packet in packets {
            queue
                .push_back(packet)
                .expect("test queue holds the packets");
        }
        let first = queue.pop_front().expect("a first packet");
        let mut buf = [0xa5; 1024];
        let (used, unsent) =
            pack_queued(first, || queue.pop_front(), encode, &mut buf, TARGET).expect("packs");
        (buf, used, unsent, queue.len())
    }

    /// Each decoded packet as its id and the timestamps of its IMU reports,
    /// counted down from `u32::MAX`.
    fn decode(mut transfer: &[u8]) -> heapless::Vec<(u8, heapless::Vec<u32, 8>), 8> {
        let mut packets = heapless::Vec::new();
        while !transfer.is_empty() {
            let (packet, rest): (Packet, _) =
                postcard::take_from_bytes(transfer).expect("whole packets back to back");
            let reports = packet.data.accel_reports().unwrap_or_default();
            let numbers = reports.iter().map(|report| u32::MAX - report.timestamp);
            packets
                .push((packet.id, numbers.collect()))
                .expect("few packets");
            transfer = rest;
        }
        packets
    }

    fn numbers<const N: usize>(numbers: [u32; N]) -> heapless::Vec<u32, 8> {
        heapless::Vec::from_slice(&numbers).expect("at most eight")
    }

    #[test]
    fn eight_queued_imu_reports_leave_as_one_batch_and_the_ninth_waits() {
        let (buf, used, unsent, left) =
            pack(core::array::from_fn::<_, 9, _>(|n| accel(7, n as u32)));
        assert_eq!(used, 235);
        assert_eq!(
            decode(&buf[..used]),
            [(7, numbers([0, 1, 2, 3, 4, 5, 6, 7]))]
        );
        let unsent = unsent.expect("the ninth report starts the next transfer");
        assert_eq!(
            decode(&postcard::to_stdvec(&unsent).expect("encodes")),
            [(7, numbers([8]))]
        );
        assert_eq!(left, 0);
    }

    #[test]
    fn a_batch_that_would_pass_the_target_keeps_its_bytes_and_holds_the_report() {
        let (buf, used, unsent, left) = pack([
            markers(2),
            accel(7, 0),
            accel(7, 1),
            accel(7, 2),
            accel(7, 3),
            accel(7, 4),
            accel(7, 5),
        ]);
        assert_eq!(used, 98 + 119);
        assert_eq!(
            decode(&buf[..used]),
            [(2, numbers([])), (7, numbers([0, 1, 2, 3]))]
        );
        let unsent = unsent.expect("the fifth report did not fit");
        assert_eq!(
            decode(&postcard::to_stdvec(&unsent).expect("encodes")),
            [(7, numbers([4]))]
        );
        assert_eq!(left, 1, "nothing behind the held report is taken");
    }

    #[test]
    fn imu_reports_do_not_cross_other_packets_or_join_another_id() {
        let (buf, used, unsent, _) = pack([accel(7, 0), markers(2), accel(7, 1), accel(9, 2)]);
        assert_eq!(
            decode(&buf[..used]),
            [
                (7, numbers([0])),
                (2, numbers([])),
                (7, numbers([1])),
                (9, numbers([2]))
            ]
        );
        assert!(unsent.is_none());
    }

    #[test]
    fn a_first_packet_at_the_target_takes_nothing_from_the_queue() {
        let mut buf = [0; 64];
        let packed = pack_queued(
            accel(7, 0),
            || panic!("the queue must stay untouched"),
            encode,
            &mut buf,
            31,
        )
        .expect("packs");
        assert!(matches!(packed, (31, None)));

        pack_queued(accel(7, 0), || None, encode, &mut [0; 30], TARGET)
            .expect_err("the first packet does not fit the buffer");
    }
}
