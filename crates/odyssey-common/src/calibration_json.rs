//! OpenCV calibration files. Read and write failures, I/O included, are
//! reported as [`serde_json::Error`].

use std::io::{Read, Write};

use nalgebra::{Isometry3, RealField};
use opencv_ros_camera::RosOpenCvIntrinsics;

use crate::ocv_types;
/// Read camera intrinsics from OpenCV calibration JSON.
///
/// Since: Odyssey Desktop 0.6.0.
///
/// # Errors
/// Returns an error when JSON cannot be read or parsed.
pub fn get_intrinsics_from_opencv_camera_calibration_json<
    R: RealField + serde::de::DeserializeOwned + Copy,
>(
    reader: impl Read,
) -> Result<RosOpenCvIntrinsics<R>, serde_json::Error> {
    let parsed: ocv_types::CameraCalibrationParams<R> = serde_json::from_reader(reader)?;
    Ok(parsed.into())
}

/// Read stereo isometry from OpenCV calibration JSON.
///
/// Since: Odyssey Desktop 0.6.0.
///
/// # Errors
/// Returns an error when JSON cannot be read or parsed.
pub fn get_isometry_from_opencv_stereo_calibration_json<
    R: RealField + serde::de::DeserializeOwned + Copy,
>(
    reader: impl Read,
) -> Result<Isometry3<R>, serde_json::Error> {
    let parsed: ocv_types::StereoCalibrationParams<R> = serde_json::from_reader(reader)?;
    Ok(parsed.into())
}

/// Write minimal camera calibration JSON.
///
/// Since: Odyssey Desktop 0.6.0.
///
/// # Errors
/// Returns an error when JSON cannot be serialized or written.
pub fn write_opencv_minimal_camera_calibration_json<R: RealField + serde::Serialize>(
    intrinsics: &RosOpenCvIntrinsics<R>,
    writer: impl Write,
) -> Result<(), serde_json::Error> {
    serde_json::to_writer_pretty(
        writer,
        &ocv_types::MinimalCameraCalibrationParams::from(intrinsics.clone()),
    )
}

/// Write minimal stereo calibration JSON.
///
/// Since: Odyssey Desktop 0.6.0.
///
/// # Errors
/// Returns an error when JSON cannot be serialized or written.
pub fn write_opencv_minimal_stereo_calibration_json<R: RealField + serde::Serialize>(
    stereo_iso: &Isometry3<R>,
    writer: impl Write,
) -> Result<(), serde_json::Error> {
    serde_json::to_writer_pretty(
        writer,
        &ocv_types::MinimalStereoCalibrationParams::from(stereo_iso.clone()),
    )
}
