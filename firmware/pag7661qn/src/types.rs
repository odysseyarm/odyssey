use arbitrary_int::*;
use bitbybit::bitfield;
use bytemuck::{Pod, Zeroable};

#[bitfield(u64, default = 0)]
#[derive(Pod, Zeroable, Debug)]
pub struct Object {
    #[bits(4..=7, rw)]
    pub id: u4,
    /// Brightness
    #[bits(8..=15, rw)]
    pub avg: u8,
    #[bits(16..=31, rw)]
    pub x: u16,
    #[bits(32..=47, rw)]
    pub y: u16,
    #[bits([48..=63, 0], rw)]
    pub area: u17,
}

#[bitfield(u8, default = 0)]
pub struct IntOStatus {
    #[bit(0, rw)]
    pub power_on_ready: bool,
    #[bit(1, rw)]
    pub frame_ready: bool,
    #[bit(7, rw)]
    pub error: bool,
}
