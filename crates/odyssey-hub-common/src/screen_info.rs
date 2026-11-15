/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ScreenInfo {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub id: u8,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub bounds: [nalgebra::Vector2<f32>; 4],
}
