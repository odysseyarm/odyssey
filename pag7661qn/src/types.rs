use arbitrary_int::*;
use bitbybit::bitfield;
use bytemuck::{Pod, Zeroable};

#[bitfield(u64, default = 0)]
#[derive(Pod, Zeroable, Debug)]
pub struct Object {
    #[bits(4..=7, rw)]
    id: u4,
    /// Brightness
    #[bits(8..=15, rw)]
    avg: u8,
    #[bits(16..=31, rw)]
    x: u16,
    #[bits(32..=47, rw)]
    y: u16,
    #[bits([48..=63, 0], rw)]
    area: u17,
}
