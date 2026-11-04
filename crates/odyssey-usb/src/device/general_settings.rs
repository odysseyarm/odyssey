use anyhow::{Result, anyhow, bail};
use nalgebra::Isometry3;
use odyssey_common::ocv_types::{MinimalCameraCalibrationParams, OpenCVMatrix3, OpenCVMatrix5x1};
use opencv_ros_camera::RosOpenCvIntrinsics;

use crate::packets::vm::{AccelConfig, ConfigKind, GeneralConfig, GyroConfig};

/// Default impact threshold of a device that has none stored.
const DEFAULT_IMPACT_THRESHOLD: u8 = 100;
/// Default time, in milliseconds, impacts are suppressed after one registers.
const DEFAULT_SUPPRESS_MS: u8 = 100;

/// Pinhole parameters used until a device reports its calibration: a 500 px
/// focal length, principal point at (320, 240), no distortion.
fn default_camera_intrinsics() -> RosOpenCvIntrinsics<f32> {
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

/// Every configuration value of a device in one struct. The device protocol
/// hands them out one [`ConfigKind`] at a time.
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
            impact_threshold: DEFAULT_IMPACT_THRESHOLD,
            suppress_ms: DEFAULT_SUPPRESS_MS,
            accel_config: AccelConfig::default(),
            gyro_config: GyroConfig::default(),
            camera_model_nf: default_camera_intrinsics(),
            camera_model_wf: default_camera_intrinsics(),
            stereo_iso: Isometry3::identity(),
        }
    }
}

impl GeneralSettings {
    /// Reads every setting with `read_config`, which asks the device for one
    /// [`ConfigKind`] over whichever channel the caller has.
    pub(super) async fn read_with<Read>(
        mut read_config: impl FnMut(ConfigKind) -> Read,
    ) -> Result<Self>
    where
        Read: Future<Output = Result<GeneralConfig>>,
    {
        let impact_threshold = read_config(ConfigKind::ImpactThreshold).await?;
        let suppress_ms = read_config(ConfigKind::SuppressMs).await?;
        let accel_config = read_config(ConfigKind::AccelConfig).await?;
        let gyro_config = read_config(ConfigKind::GyroConfig).await?;
        let camera_model_nf = read_config(ConfigKind::CameraModelNf).await?;
        let camera_model_wf = read_config(ConfigKind::CameraModelWf).await?;
        let stereo_iso = read_config(ConfigKind::StereoIso).await?;

        let GeneralConfig::ImpactThreshold(impact_threshold) = impact_threshold else {
            bail!("Unexpected config variant for ImpactThreshold");
        };
        let GeneralConfig::SuppressMs(suppress_ms) = suppress_ms else {
            bail!("Unexpected config variant for SuppressMs");
        };
        let GeneralConfig::AccelConfig(accel_config) = accel_config else {
            bail!("Unexpected config variant for AccelConfig");
        };
        let GeneralConfig::GyroConfig(gyro_config) = gyro_config else {
            bail!("Unexpected config variant for GyroConfig");
        };
        let GeneralConfig::CameraModelNf(camera_model_nf) = camera_model_nf else {
            bail!("Unexpected config variant for CameraModelNf");
        };
        let GeneralConfig::CameraModelWf(camera_model_wf) = camera_model_wf else {
            bail!("Unexpected config variant for CameraModelWf");
        };
        let GeneralConfig::StereoIso(stereo_iso) = stereo_iso else {
            bail!("Unexpected config variant for StereoIso");
        };

        Ok(Self {
            impact_threshold,
            suppress_ms,
            accel_config,
            gyro_config,
            camera_model_nf: camera_intrinsics_from_wire(camera_model_nf)?,
            camera_model_wf: camera_intrinsics_from_wire(camera_model_wf)?,
            stereo_iso,
        })
    }
}

fn camera_intrinsics_from_wire(
    pod: opencv_ros_camera::RosOpenCvIntrinsicsPod<f32>,
) -> Result<RosOpenCvIntrinsics<f32>> {
    RosOpenCvIntrinsics::try_from(pod).map_err(|e| anyhow!("invalid camera model: {e}"))
}
