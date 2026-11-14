use serde::Serialize;

/// Since: Odyssey Desktop 0.6.0.
#[derive(Clone, Debug, Serialize)]
pub struct TestFrame {
    /// Since: Odyssey Desktop 0.6.0.
    pub fv_aimpoint_x: Option<f32>,
    /// Since: Odyssey Desktop 0.6.0.
    pub fv_aimpoint_y: Option<f32>,
    /// Since: Odyssey Desktop 0.6.0.
    pub opposite_cant: Option<f32>,
    /// Since: Odyssey Desktop 0.6.0.
    pub position_x: Option<f32>,
    /// Since: Odyssey Desktop 0.6.0.
    pub position_y: Option<f32>,
    /// Since: Odyssey Desktop 0.6.0.
    pub position_z: Option<f32>,
}
