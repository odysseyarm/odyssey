use nalgebra::RealField;
use num::FromPrimitive;
use opencv_ros_camera::{Distortion, RosOpenCvIntrinsics};

pub fn ros_opencv_intrinsics_type_convert<A: simba::scalar::SubsetOf<B>, B: From<A>>(
    intrinsics: &RosOpenCvIntrinsics<A>,
) -> RosOpenCvIntrinsics<B>
where
    A: RealField + Copy + FromPrimitive,
    B: RealField + Copy + FromPrimitive,
{
    let _0 = B::from_f32(0.).unwrap();

    let fx = intrinsics.p.m11.into();
    let fy = intrinsics.p.m22.into();
    let cx = intrinsics.p.m13.into();
    let cy = intrinsics.p.m23.into();

    let distortion = Distortion::from_opencv_vec(intrinsics.distortion.opencv_vec().cast());

    RosOpenCvIntrinsics::from_params_with_distortion(fx, _0, fy, cx, cy, distortion)
}
