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
