use std::num::NonZero;

use serde::{Deserialize, Serialize};

pub mod config;
pub mod device;
pub mod events;
pub mod accessory;
mod hexkeymap;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScreenInfo {
    pub id: u8,
    pub bounds: [nalgebra::Vector2<f32>; 4],
}
