use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct TestFrame {
    pub fv_aimpoint_x: Option<f32>,
    pub fv_aimpoint_y: Option<f32>,
    pub opposite_cant: Option<f32>,
    pub position_x: Option<f32>,
    pub position_y: Option<f32>,
    pub position_z: Option<f32>,
}
