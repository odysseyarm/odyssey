use std::error::Error;
use std::io::{Read, Write};

use nalgebra::{Isometry3, RealField};
use opencv_ros_camera::RosOpenCvIntrinsics;

use crate::ocv_types;

pub fn get_intrinsics_from_opencv_camera_calibration_json<
    R: RealField + serde::de::DeserializeOwned + Copy,
>(
    mut reader: impl Read,
) -> Result<RosOpenCvIntrinsics<R>, Box<dyn Error>> {
    let mut contents = String::new();
    reader.read_to_string(&mut contents)?;
    let parsed_json: ocv_types::CameraCalibrationParams<R> = serde_json::from_str(&contents)?;

    Ok(parsed_json.into())
}

pub fn get_isometry_from_opencv_stereo_calibration_json<
    R: RealField + serde::de::DeserializeOwned + Copy,
>(
    mut reader: impl Read,
) -> Result<Isometry3<R>, Box<dyn Error>> {
    let mut contents = String::new();
    reader.read_to_string(&mut contents)?;
    let parsed_json: ocv_types::StereoCalibrationParams<R> = serde_json::from_str(&contents)?;

    Ok(parsed_json.into())
}

pub fn write_opencv_minimal_camera_calibration_json<R: RealField + serde::Serialize>(
    intrinsics: &RosOpenCvIntrinsics<R>,
    mut writer: impl Write,
) -> Result<(), Box<dyn Error>> {
    let json = serde_json::to_string_pretty(&ocv_types::MinimalCameraCalibrationParams::from(
        intrinsics.clone(),
    ))?;
    writer.write_all(json.as_bytes())?;
    Ok(())
}

pub fn write_opencv_minimal_stereo_calibration_json<R: RealField + serde::Serialize>(
    stereo_iso: &Isometry3<R>,
    mut writer: impl Write,
) -> Result<(), Box<dyn Error>> {
    let json = serde_json::to_string_pretty(&ocv_types::MinimalStereoCalibrationParams::from(
        stereo_iso.clone(),
    ))?;
    writer.write_all(json.as_bytes())?;
    Ok(())
}
