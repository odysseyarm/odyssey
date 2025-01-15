#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

use nalgebra::{
    Isometry3, RealField,
    Scalar,
};
#[cfg(feature = "std")]
use opencv_ros_camera::{Distortion, RosOpenCvIntrinsics};
#[cfg(feature = "std")]
use std::error::Error;
#[cfg(feature = "std")]
use std::io::Read;
use num::{
    traits::Float,
    FromPrimitive,
};

pub mod ocv_types;

#[cfg(feature = "std")]
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

#[cfg(feature = "std")]
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

#[cfg(feature = "std")]
pub fn write_opencv_minimal_camera_calibration_json<R: RealField + serde::Serialize>(
    intrinsics: &RosOpenCvIntrinsics<R>,
    mut writer: impl std::io::Write,
) -> Result<(), Box<dyn Error>> {
    let json = serde_json::to_string_pretty(&ocv_types::MinimalCameraCalibrationParams::from(
        intrinsics.clone(),
    ))?;
    writer.write_all(json.as_bytes())?;
    Ok(())
}

#[cfg(feature = "std")]
pub fn write_opencv_minimal_stereo_calibration_json<R: RealField + serde::Serialize>(
    stereo_iso: &Isometry3<R>,
    mut writer: impl std::io::Write,
) -> Result<(), Box<dyn Error>> {
    let json = serde_json::to_string_pretty(&ocv_types::MinimalStereoCalibrationParams::from(
        stereo_iso.clone(),
    ))?;
    writer.write_all(json.as_bytes())?;
    Ok(())
}

#[cfg(feature = "std")]
pub fn ros_opencv_intrinsics_type_convert<A: simba::scalar::SubsetOf<B>, B: From<A>>(
    intrinsics: &RosOpenCvIntrinsics<A>,
) -> RosOpenCvIntrinsics<B>
where
    A: RealField + Copy + FromPrimitive,
    B: RealField + Copy + FromPrimitive,
{
    use opencv_ros_camera::RosOpenCvIntrinsics;

    let _0 = B::from_f32(0.).unwrap();

    let fx = intrinsics.p.m11.into();
    let fy = intrinsics.p.m22.into();
    let cx = intrinsics.p.m13.into();
    let cy = intrinsics.p.m23.into();

    let distortion = Distortion::from_opencv_vec(intrinsics.distortion.opencv_vec().cast());

    RosOpenCvIntrinsics::from_params_with_distortion(fx, _0, fy, cx, cy, distortion)
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy)]
pub struct Plane<F: Float + FromPrimitive + Scalar> {
    pub origin: nalgebra::Point3<F>,
    pub normal: nalgebra::Vector3<F>,
}

impl<F: Float + FromPrimitive + Scalar> Plane<F> {
    pub fn cast<G>(&self) -> Plane<G>
    where
        G: Float + FromPrimitive + Scalar + nalgebra::RealField,
        F: simba::scalar::SubsetOf<G>,
    {
        Plane {
            origin: self.origin.cast(),
            normal: self.normal.cast(),
        }
    }
}
