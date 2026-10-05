//! The wire form of the camera model is hand-mapped onto
//! [`RosOpenCvIntrinsicsPod`] instead of going through
//! `RosOpenCvIntrinsics::from_params_with_distortion`, so pin the two against
//! each other: a wrong row/column-major shuffle here silently mis-calibrates a
//! device.

use opencv_ros_camera::{RosOpenCvIntrinsics, RosOpenCvIntrinsicsPod};
use odyssey_protocol::wire::CameraCalibrationParams;

const FX: f32 = 1234.5;
const SKEW: f32 = 0.25;
const CX: f32 = 639.5;
const FY: f32 = 1230.0;
const CY: f32 = 359.5;
const DIST: [f32; 5] = [-0.3, 0.1, 0.001, -0.002, 0.05];

/// OpenCV's `K`, row-major, which is what the device sends.
fn wire(dist_coeffs: [f32; 5]) -> CameraCalibrationParams {
    CameraCalibrationParams {
        camera_matrix: [FX, SKEW, CX, 0.0, FY, CY, 0.0, 0.0, 1.0],
        dist_coeffs,
    }
}

#[test]
fn wire_to_pod_matches_upstream_construction() {
    let reference = RosOpenCvIntrinsics::from_params(FX, SKEW, FY, CX, CY);

    assert_eq!(
        RosOpenCvIntrinsicsPod::<f32>::from(wire([0.0; 5])),
        RosOpenCvIntrinsicsPod::from(&reference),
    );
}

#[test]
fn pod_round_trips_through_the_wire_form() {
    let pod = RosOpenCvIntrinsicsPod::<f32>::from(wire(DIST));
    assert_eq!(CameraCalibrationParams::from(pod), wire(DIST));
}

#[test]
fn pod_rebuilds_a_usable_camera() {
    let pod = RosOpenCvIntrinsicsPod::<f32>::from(wire(DIST));
    let cam = RosOpenCvIntrinsics::try_from(pod).expect("rect is the identity, so invertible");

    assert_eq!(cam.k[(0, 0)], FX);
    assert_eq!(cam.k[(0, 1)], SKEW);
    assert_eq!(cam.k[(0, 2)], CX);
    assert_eq!(cam.k[(1, 1)], FY);
    assert_eq!(cam.k[(1, 2)], CY);
    assert_eq!(cam.distortion.opencv_vec().as_slice(), DIST);
}
