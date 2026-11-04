use nalgebra::Isometry3;
use opencv_ros_camera::RosOpenCvIntrinsics;

use crate::packets::vm::{AccelConfig, GyroConfig};

fn default_camera_intrinsics() -> RosOpenCvIntrinsics<f32> {
    // Create a default camera with identity-like parameters
    // Using a simple pinhole camera model with no distortion
    use odyssey_common::ocv_types::{
        MinimalCameraCalibrationParams, OpenCVMatrix3, OpenCVMatrix5x1,
    };
    MinimalCameraCalibrationParams {
        camera_matrix: OpenCVMatrix3 {
            data: [
                500.0, 0.0, 320.0, // fx, 0, cx
                0.0, 500.0, 240.0, // 0, fy, cy
                0.0, 0.0, 1.0, // 0, 0, 1
            ],
        },
        dist_coeffs: OpenCVMatrix5x1 {
            data: [0.0, 0.0, 0.0, 0.0, 0.0], // no distortion
        },
    }
    .into()
}

/// A struct that holds all configuration values together
/// This is needed because the new protodonge API returns individual config items as enum variants
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GeneralSettings {
    pub impact_threshold: u8,
    pub suppress_ms: u8,
    pub accel_config: AccelConfig,
    pub gyro_config: GyroConfig,
    pub camera_model_nf: RosOpenCvIntrinsics<f32>,
    pub camera_model_wf: RosOpenCvIntrinsics<f32>,
    pub stereo_iso: Isometry3<f32>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            impact_threshold: 100,
            suppress_ms: 100,
            accel_config: AccelConfig::default(),
            gyro_config: GyroConfig::default(),
            camera_model_nf: default_camera_intrinsics(),
            camera_model_wf: default_camera_intrinsics(),
            stereo_iso: Isometry3::identity(),
        }
    }
}
