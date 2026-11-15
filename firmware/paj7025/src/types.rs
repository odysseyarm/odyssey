use arbitrary_int::*;
use bitbybit::bitfield;
use bytemuck::{Pod, Zeroable};

/// Since: Odyssey Desktop 0.6.0.
#[bitfield(u128, default = 0, debug)]
#[derive(Pod, Zeroable)]
pub struct ObjectFormat1 {
    /// Since: Odyssey Desktop 0.6.0.
    #[bits(0..=13, rw)]
    pub area: u14,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(16..=27, rw)]
    pub cx: u12,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(32..=43, rw)]
    pub cy: u12,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(48..=55, rw)]
    pub avg_brightness: u8,
    /// Since: Odyssey Desktop 0.6.0.
    #[bits(56..=63, rw)]
    pub max_brightness: u8,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(64..=67, rw)]
    pub radius: u4,
    /// Since: Odyssey Desktop 0.6.0.
    #[bits(68..=71, rw)]
    pub range: u4,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(72..=78, rw)]
    pub boundary_left: u7,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(80..=86, rw)]
    pub boundary_right: u7,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(88..=94, rw)]
    pub boundary_up: u7,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(96..=102, rw)]
    pub boundary_down: u7,

    /// Since: Odyssey Desktop 0.6.0.
    #[bits(104..=111, rw)]
    pub aspect_ratio: u8,
    /// Since: Odyssey Desktop 0.6.0.
    #[bits(112..=119, rw)]
    pub vx: i8,
    /// Since: Odyssey Desktop 0.6.0.
    #[bits(120..=127, rw)]
    pub vy: i8,
}
