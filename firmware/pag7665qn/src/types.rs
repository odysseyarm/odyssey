use arbitrary_int::*;
use bitbybit::bitfield;
use bytemuck::{Pod, Zeroable};

/// Object detection result from PAG7665QN.
///
/// The PAG7665QN supports up to 16 objects per frame with circle detection.
/// Each object is 8 bytes with the following layout:
/// - Byte 0: bits[7:4] = ID, bits[1] = IsCircle, bit[0] = Area[16]
/// - Byte 1: Avg (brightness)
/// - Bytes 2-3: X coordinate (16-bit, 6-bit fractional)
/// - Bytes 4-5: Y coordinate (16-bit, 6-bit fractional)
/// - Bytes 6-7: Area[15:0]
#[bitfield(u64, default = 0)]
#[derive(Pod, Zeroable, Debug)]
pub struct Object {
    /// Object ID (0-15)
    #[bits(4..=7, rw)]
    pub id: u4,
    /// Circle detection flag - true if object shape is approximately circular
    #[bit(1, rw)]
    pub is_circle: bool,
    /// Brightness (average pixel value of the object)
    #[bits(8..=15, rw)]
    pub avg: u8,
    /// X coordinate with 6-bit fractional precision (divide by 64 for pixels)
    #[bits(16..=31, rw)]
    pub x: u16,
    /// Y coordinate with 6-bit fractional precision (divide by 64 for pixels)
    #[bits(32..=47, rw)]
    pub y: u16,
    /// Object area in pixels. Bit 16 is at bit 0 of byte 0, bits 15:0 are in bytes 6-7.
    #[bits([48..=63, 0], rw)]
    pub area: u17,
}

/// INTO_Status register (0x04 in Bank 0x00)
#[bitfield(u8, default = 0)]
pub struct IntOStatus {
    /// Power-on ready flag
    #[bit(0, rw)]
    pub power_on_ready: bool,
    /// Frame ready flag (image mode) or object ready flag (object mode)
    #[bit(1, rw)]
    pub frame_ready: bool,
    /// Error flag - check FW_Error_Type and FW_Error_Code registers
    #[bit(7, rw)]
    pub error: bool,
}

/// ROI (Region of Interest) configuration
#[derive(Debug, Clone, Copy, Default)]
pub struct Roi {
    /// Enable ROI
    pub enabled: bool,
    /// X coordinate of top-left corner (0-319)
    pub x0: u16,
    /// Y coordinate of top-left corner (0-239)
    pub y0: u8,
    /// X coordinate of bottom-right corner (0-319)
    pub x1: u16,
    /// Y coordinate of bottom-right corner (0-239)
    pub y1: u8,
}

/// Circle detection parameters
#[derive(Debug, Clone, Copy)]
pub struct CircleParams {
    /// R ratio lower bound (width/height ratio, default 0x1A = 0.8125)
    pub r_lower_bound: u8,
    /// R ratio upper bound (width/height ratio, default 0x26 = 1.1875)
    pub r_upper_bound: u8,
    /// K ratio lower bound (area/rect_area ratio, default 0x17 = 0.71875)
    pub k_lower_bound: u8,
    /// K ratio upper bound (area/rect_area ratio, default 0x1D = 0.90625)
    pub k_upper_bound: u8,
}

impl Default for CircleParams {
    fn default() -> Self {
        Self {
            r_lower_bound: 0x1A,
            r_upper_bound: 0x26,
            k_lower_bound: 0x17,
            k_upper_bound: 0x1D,
        }
    }
}
