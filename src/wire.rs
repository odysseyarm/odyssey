use ats_common::ocv_types::{
    MinimalCameraCalibrationParams, MinimalStereoCalibrationParams, OpenCVMatrix3, OpenCVMatrix3x1,
    OpenCVMatrix5x1,
};
#[allow(unused_imports)]
use nalgebra::ComplexField;
use opencv_ros_camera::RosOpenCvIntrinsics;

#[cfg(feature = "minicbor")]
use minicbor::{CborLen, Decode, Encode};

#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode, CborLen))]
pub struct AccelConfig {
    #[cfg(feature = "serde")]
    #[serde(default = "default_accel_odr")]
    #[cfg_attr(feature = "minicbor", n(0))]
    pub accel_odr: u16,
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg(not(feature = "serde"))]
    pub accel_odr: u16,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub b_x: f32,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub b_y: f32,
    #[cfg_attr(feature = "minicbor", n(4))]
    pub b_z: f32,
    #[cfg_attr(feature = "minicbor", n(5))]
    pub s_x: f32,
    #[cfg_attr(feature = "minicbor", n(6))]
    pub s_y: f32,
    #[cfg_attr(feature = "minicbor", n(7))]
    pub s_z: f32,
}

#[cfg(feature = "serde")]
fn default_accel_odr() -> u16 {
    100
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

#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode, CborLen))]
pub struct CameraCalibrationParams {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub camera_matrix: [f32; 9],
    #[cfg_attr(feature = "minicbor", n(1))]
    pub dist_coeffs: [f32; 5],
}

impl From<CameraCalibrationParams> for RosOpenCvIntrinsics<f32> {
    fn from(value: CameraCalibrationParams) -> Self {
        MinimalCameraCalibrationParams {
            camera_matrix: OpenCVMatrix3 {
                data: value.camera_matrix,
            },
            dist_coeffs: OpenCVMatrix5x1 {
                data: value.dist_coeffs,
            },
        }
        .into()
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

#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode, CborLen))]
pub struct StereoCalibrationParams {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub r: [f32; 9],
    #[cfg_attr(feature = "minicbor", n(1))]
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
        }
        .into()
    }
}

impl From<nalgebra::Isometry3<f32>> for StereoCalibrationParams {
    fn from(value: nalgebra::Isometry3<f32>) -> Self {
        let value = MinimalStereoCalibrationParams::from(value);
        Self {
            r: value.r.data,
            t: value.t.data,
        }
    }
}

// This is also the format the POC uses on flash
#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode, CborLen))]
pub enum GeneralConfig {
    #[cfg_attr(feature = "minicbor", n(0))]
    ImpactThreshold(#[cfg_attr(feature = "minicbor", n(0))] u8),
    #[cfg_attr(feature = "minicbor", n(1))]
    SuppressMs(#[cfg_attr(feature = "minicbor", n(0))] u8),
    #[cfg_attr(feature = "minicbor", n(2))]
    AccelConfig(#[cfg_attr(feature = "minicbor", n(0))] AccelConfig),
    #[cfg_attr(feature = "minicbor", n(3))]
    GyroConfig(#[cfg_attr(feature = "minicbor", n(0))] super::GyroConfig),
    #[cfg_attr(feature = "minicbor", n(4))]
    CameraModelNf(#[cfg_attr(feature = "minicbor", n(0))] CameraCalibrationParams),
    #[cfg_attr(feature = "minicbor", n(5))]
    CameraModelWf(#[cfg_attr(feature = "minicbor", n(0))] CameraCalibrationParams),
    #[cfg_attr(feature = "minicbor", n(6))]
    StereoIso(#[cfg_attr(feature = "minicbor", n(0))] StereoCalibrationParams),
}

impl From<super::GeneralConfig> for GeneralConfig {
    fn from(value: super::GeneralConfig) -> Self {
        match value {
            super::GeneralConfig::ImpactThreshold(x) => Self::ImpactThreshold(x),
            super::GeneralConfig::SuppressMs(x) => Self::SuppressMs(x),
            super::GeneralConfig::AccelConfig(x) => Self::AccelConfig(x.into()),
            super::GeneralConfig::GyroConfig(x) => Self::GyroConfig(x),
            super::GeneralConfig::CameraModelNf(x) => Self::CameraModelNf(x.into()),
            super::GeneralConfig::CameraModelWf(x) => Self::CameraModelWf(x.into()),
            super::GeneralConfig::StereoIso(x) => Self::StereoIso(x.into()),
        }
    }
}

impl From<GeneralConfig> for super::GeneralConfig {
    fn from(value: GeneralConfig) -> Self {
        match value {
            GeneralConfig::ImpactThreshold(x) => Self::ImpactThreshold(x),
            GeneralConfig::SuppressMs(x) => Self::SuppressMs(x),
            GeneralConfig::AccelConfig(x) => Self::AccelConfig(x.into()),
            GeneralConfig::GyroConfig(x) => Self::GyroConfig(x),
            GeneralConfig::CameraModelNf(x) => Self::CameraModelNf(x.into()),
            GeneralConfig::CameraModelWf(x) => Self::CameraModelWf(x.into()),
            GeneralConfig::StereoIso(x) => Self::StereoIso(x.into()),
        }
    }
}

#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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
