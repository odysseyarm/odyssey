use std::num::NonZero;

use serde::{Deserialize, Serialize};

pub mod config;
pub mod device;
pub mod events;
mod hexkeymap;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScreenInfo {
    pub id: u8,
    pub bounds: [nalgebra::Vector2<f32>; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum AccessoryType {
    DryFireMag,
}

#[repr(C)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccessoryInfo {
    pub name: String,
    pub ty: AccessoryType,
    pub assignment: Option<NonZero<u64>>,
}

pub type AccessoryConnected = bool;
pub type AccessoryMap = std::collections::HashMap<[u8; 6], (AccessoryInfo, AccessoryConnected)>;
