use ats_common::ocv_types::{MinimalCameraCalibrationParams, MinimalStereoCalibrationParams, OpenCVMatrix3, OpenCVMatrix3x1, OpenCVMatrix5x1};
use bytemuck::{AnyBitPattern, NoUninit};
use opencv_ros_camera::RosOpenCvIntrinsics;
#[allow(unused_imports)]
use nalgebra::ComplexField;

#[derive(Clone, Copy, NoUninit, AnyBitPattern)]
#[repr(C, packed)]
pub struct AccelConfig {
    pub accel_odr: u16,
    pub b_x: f32,
    pub b_y: f32,
    pub b_z: f32,
    pub s_x: f32,
    pub s_y: f32,
    pub s_z: f32,
}

impl Default for AccelConfig {
    fn default() -> Self {
        Self {
            accel_odr: 200,
            b_x: 0.0,
            b_y: 0.0,
            b_z: 0.0,
            s_x: 1.0,
            s_y: 1.0,
            s_z: 1.0,
        }
    }
}

impl From<super::AccelConfig> for AccelConfig {
    fn from(value: super::AccelConfig) -> Self {
        Self {
            accel_odr: value.accel_odr,
            b_x: value.b_x,
            b_y: value.b_y,
            b_z: value.b_z,
            s_x: value.s_x,
            s_y: value.s_y,
            s_z: value.s_z,
        }
    }
}

impl From<AccelConfig> for super::AccelConfig {
    fn from(value: AccelConfig) -> Self {
        Self {
            accel_odr: value.accel_odr,
            b_x: value.b_x,
            b_y: value.b_y,
            b_z: value.b_z,
            s_x: value.s_x,
            s_y: value.s_y,
            s_z: value.s_z,
        }
    }
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Clone, Copy, NoUninit, AnyBitPattern)]
#[repr(C)]
pub struct CameraCalibrationParams {
    pub camera_matrix: [f32; 9],
    pub dist_coeffs: [f32; 5],
}

impl From<CameraCalibrationParams> for RosOpenCvIntrinsics<f32> {
    fn from(value: CameraCalibrationParams) -> Self {
        MinimalCameraCalibrationParams {
            camera_matrix: OpenCVMatrix3 { data: value.camera_matrix },
            dist_coeffs: OpenCVMatrix5x1 { data: value.dist_coeffs },
        }.into()
    }
}

impl From<RosOpenCvIntrinsics<f32>> for CameraCalibrationParams {
    fn from(value: RosOpenCvIntrinsics<f32>) -> Self {
        let value = MinimalCameraCalibrationParams::from(value);
        Self {
            camera_matrix: value.camera_matrix.data,
            dist_coeffs: value.dist_coeffs.data,
        }
    }
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Clone, Copy, NoUninit, AnyBitPattern)]
#[repr(C)]
pub struct StereoCalibrationParams {
    pub r: [f32; 9],
    pub t: [f32; 3],
}

impl Default for StereoCalibrationParams {
    fn default() -> Self {
        Self {
            r: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            t: [0.0, 0.0, 0.0],
        }
    }
}

impl From<StereoCalibrationParams> for nalgebra::Isometry3<f32> {
    fn from(value: StereoCalibrationParams) -> Self {
        MinimalStereoCalibrationParams {
            r: OpenCVMatrix3 { data: value.r },
            t: OpenCVMatrix3x1 { data: value.t },
        }.into()
    }
}

impl From<nalgebra::Isometry3<f32>> for StereoCalibrationParams{
    fn from(value: nalgebra::Isometry3<f32>) -> Self {
        let value = MinimalStereoCalibrationParams::from(value);
        Self {
            r: value.r.data,
            t: value.t.data,
        }
    }
}


// This is also the format the POC uses on flash
#[derive(Clone, Copy, NoUninit, AnyBitPattern)]
#[repr(C, packed)]
pub struct GeneralConfig {
    pub impact_threshold: u8,
    pub accel_config: AccelConfig,
    pub gyro_config: super::GyroConfig,
    pub camera_model_nf: CameraCalibrationParams,
    pub camera_model_wf: CameraCalibrationParams,
    pub stereo_iso: StereoCalibrationParams,
}

impl From<super::GeneralConfig> for GeneralConfig {
    fn from(value: super::GeneralConfig) -> Self {
        Self {
            impact_threshold: value.impact_threshold,
            accel_config: value.accel_config.into(),
            gyro_config: value.gyro_config,
            camera_model_nf: value.camera_model_nf.into(),
            camera_model_wf: value.camera_model_wf.into(),
            stereo_iso: value.stereo_iso.into(),
        }
    }
}

impl From<GeneralConfig> for super::GeneralConfig {
    fn from(value: GeneralConfig) -> Self {
        let mut r = Self {
            impact_threshold: value.impact_threshold,
            accel_config: value.accel_config.into(),
            gyro_config: value.gyro_config,
            camera_model_nf: value.camera_model_nf.into(),
            camera_model_wf: value.camera_model_wf.into(),
            stereo_iso: value.stereo_iso.into(),
        };

        // HACK old configs use camera calibration based on 98x98.
        // Check cx and rescale to 4095x4095
        if r.camera_model_nf.p.m13 < 100.0 {
            r.camera_model_nf.p.m11 *= 4095.0 / 98.0;
            r.camera_model_nf.p.m22 *= 4095.0 / 98.0;
            r.camera_model_nf.p.m13 *= 4095.0 / 98.0;
            r.camera_model_nf.p.m23 *= 4095.0 / 98.0;
        }
        if r.camera_model_wf.p.m13 < 100.0 {
            r.camera_model_wf.p.m11 *= 4095.0 / 98.0;
            r.camera_model_wf.p.m22 *= 4095.0 / 98.0;
            r.camera_model_wf.p.m13 *= 4095.0 / 98.0;
            r.camera_model_wf.p.m23 *= 4095.0 / 98.0;
        }
        r
    }
}

#[derive(Clone, Copy, NoUninit, AnyBitPattern)]
#[repr(C)]
pub(super) struct AccelReport {
    timestamp: u32,
    accel: [i16; 3],
    gyro: [i16; 3],
}

// accel: x, y, z, 2048 = 1g
// gyro: x, y, z, 16.4 = 1dps
impl From<super::AccelReport> for AccelReport {
    fn from(value: super::AccelReport) -> Self {
        Self {
            timestamp: value.timestamp,
            accel: value.accel.data.0[0].map(|a| (a / 9.806650 * 2048.0).round() as i16),
            gyro: value.gyro.data.0[0].map(|g| (g.to_degrees() * 16.4).round() as i16),
        }
    }
}

impl From<AccelReport> for super::AccelReport {
    fn from(value: AccelReport) -> Self {
        Self {
            timestamp: value.timestamp,
            accel: value.accel.map(|a| a as f32 / 2048.0 * 9.806650).into(),
            gyro: value.gyro.map(|g| (g as f32 / 16.4).to_radians()).into(),
        }
    }
}
