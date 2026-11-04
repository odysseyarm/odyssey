use nalgebra::RealField;
use num::FromPrimitive;
use opencv_ros_camera::{Distortion, RosOpenCvIntrinsics};

/// Re-expresses camera intrinsics in another scalar type, e.g. `f32` -> `f64`.
///
/// Skew is dropped: the pinhole model rebuilt here carries `fx, fy, cx, cy`
/// and the distortion coefficients only.
pub fn ros_opencv_intrinsics_type_convert<A, B>(
    intrinsics: &RosOpenCvIntrinsics<A>,
) -> RosOpenCvIntrinsics<B>
where
    A: RealField + Copy + FromPrimitive + simba::scalar::SubsetOf<B>,
    B: RealField + Copy + FromPrimitive + From<A>,
{
    let zero_skew = B::zero();

    let fx = intrinsics.p.m11.into();
    let fy = intrinsics.p.m22.into();
    let cx = intrinsics.p.m13.into();
    let cy = intrinsics.p.m23.into();

    let distortion = Distortion::from_opencv_vec(intrinsics.distortion.opencv_vec().cast());

    RosOpenCvIntrinsics::from_params_with_distortion(fx, zero_skew, fy, cx, cy, distortion)
}
