use serde::{Deserialize, Serialize};

pub mod config;
pub mod device;
pub mod events;
mod hexkeymap;

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct ScreenInfo {
    pub id: u8,
    pub bounds: [nalgebra::Vector2<f32>; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AccessoryType {
    DryFireMag,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccessoryInfo {
    pub uuid: [u8; 6],
    pub name: String,
    pub ty: AccessoryType,
}
