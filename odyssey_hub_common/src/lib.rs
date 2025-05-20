pub mod device;
pub mod events;
pub mod config;

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct ScreenInfo {
    pub id: u8,
    pub bounds: [nalgebra::Vector2<f32>; 4],
}
