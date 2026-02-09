//! COBS framing utilities for packet handling
//!
//! This module provides low-level COBS encoding/decoding utilities.
//! The actual packet parsing is done by the board-specific code using protodongers.

/// COBS decode error
#[derive(Debug, Clone, Copy)]
pub struct DecodeError;

/// Decode a COBS-encoded frame in place.
///
/// Returns the decoded length on success.
pub fn decode_in_place(buf: &mut [u8]) -> Result<usize, DecodeError> {
    cobs::decode_in_place(buf).map_err(|_| DecodeError)
}

/// Encode data with COBS framing.
///
/// Returns the encoded length.
pub fn encode(src: &[u8], dst: &mut [u8]) -> usize {
    cobs::encode(src, dst)
}

/// Helper to find the next COBS frame delimiter (0x00) in a buffer.
///
/// Returns the position of the delimiter if found.
pub fn find_delimiter(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == 0x00)
}
