use nalgebra::RealField;
use num::FromPrimitive;
use opencv_ros_camera::{Distortion, RosOpenCvIntrinsics};

/// Re-expresses camera intrinsics in another scalar type, e.g. `f32` -> `f64`.
///
/// Preserves the pinhole parameters, including skew, and distortion.
///
/// Since: Odyssey Desktop 0.6.0.
pub fn ros_opencv_intrinsics_type_convert<A, B>(
    intrinsics: &RosOpenCvIntrinsics<A>,
) -> RosOpenCvIntrinsics<B>
where
    A: RealField + Copy + FromPrimitive + simba::scalar::SubsetOf<B>,
    B: RealField + Copy + FromPrimitive + From<A>,
{
    let skew = intrinsics.p.m12.into();

    let fx = intrinsics.p.m11.into();
    let fy = intrinsics.p.m22.into();
    let cx = intrinsics.p.m13.into();
    let cy = intrinsics.p.m23.into();

    let distortion = Distortion::from_opencv_vec(intrinsics.distortion.opencv_vec().cast());

    RosOpenCvIntrinsics::from_params_with_distortion(fx, skew, fy, cx, cy, distortion)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_conversion_preserves_nonzero_skew_and_distortion() {
        let source = RosOpenCvIntrinsics::from_params_with_distortion(
            900.0_f32,
            0.75,
            910.0,
            640.0,
            360.0,
            Distortion::from_opencv_vec(nalgebra::Vector5::new(0.1, -0.2, 0.003, -0.004, 0.05)),
        );
        let converted: RosOpenCvIntrinsics<f64> = ros_opencv_intrinsics_type_convert(&source);
        assert_eq!(converted.p, source.p.cast::<f64>());
        assert_eq!(
            converted.distortion.opencv_vec(),
            &source.distortion.opencv_vec().cast::<f64>()
        );
    }
}
