use nalgebra::Matrix3;
use odyssey_common::MARKER_PATTERN_LEN;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
pub struct ScreenInfo {
    pub screen_dimensions_meters: [f32; 2],
    pub marker_points: [nalgebra::Point3<f32>; MARKER_PATTERN_LEN],
}

impl From<ScreenInfo> for odyssey_common::ScreenCalibration<f32> {
    fn from(screen_info: ScreenInfo) -> Self {
        Self {
            surface: None,
            rotation: Default::default(),
            homography: {
                Matrix3::new(
                    1. / screen_info.screen_dimensions_meters[0],
                    0.0,
                    0.0,
                    0.0,
                    1. / screen_info.screen_dimensions_meters[1],
                    0.0,
                    0.0,
                    0.0,
                    1.0,
                )
            },
            object_points: screen_info.marker_points,
        }
    }
}
