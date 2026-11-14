//! COBS framing for packets. Parsing the decoded bytes is left to the board
//! code, which uses odyssey-protocol.

/// The frame is not valid COBS.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, Clone, Copy)]
pub struct DecodeError;

/// Decode a COBS-encoded frame in place.
///
/// Returns the decoded length on success.
///
/// # Errors
///
/// Returns [`DecodeError`] when `buf` is not a valid COBS frame.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn decode_in_place(buf: &mut [u8]) -> Result<usize, DecodeError> {
    cobs::decode_in_place(buf).map_err(|_| DecodeError)
}

/// COBS-encodes `src` into `dst` and returns the encoded length. No 0x00
/// delimiter is appended.
///
/// # Panics
///
/// Panics if `dst` is shorter than `cobs::max_encoding_length(src.len())`.
///
/// # Since
/// Odyssey Desktop 0.7.0.
pub fn encode(src: &[u8], dst: &mut [u8]) -> usize {
    cobs::encode(src, dst)
}

/// Position of the first 0x00 frame delimiter in `buf`, if any.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn find_delimiter(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == 0x00)
}
