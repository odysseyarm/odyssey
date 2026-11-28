//! Packs queued reports into one BLE SDU or USB transfer.

use odyssey_protocol::Packet;

/// Bytes of packets one transfer aims for: one PDU of the BLE data channel less the two-byte
/// SDU length that precedes the payload in an SDU's first PDU. Direct USB transfers use the
/// same target.
const PACKING_TARGET: usize = crate::ble::peripheral::DATA_MPS as usize - 2;

/// Serializes `first` and the packets `queued` yields into `buf` as one transfer. Returns the
/// bytes used and the packet that did not fit, which starts the next transfer.
pub(crate) fn pack_queued(
    first: Packet,
    queued: impl FnMut() -> Option<Packet>,
    buf: &mut [u8],
) -> Result<(usize, Option<Packet>), postcard::Error> {
    let encode = |packet: &Packet, out: &mut [u8]| {
        postcard::to_slice(packet, out).map(|encoded| encoded.len())
    };
    odyssey_protocol::pack_queued(first, queued, encode, buf, PACKING_TARGET)
}
