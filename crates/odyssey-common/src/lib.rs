#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

#[cfg(feature = "std")]
mod calibration_json;
#[cfg(feature = "std")]
mod intrinsics_convert;
pub mod ocv_types;
mod plane;
mod screen_calibration;
#[cfg(feature = "std")]
mod screen_surface;

#[cfg(feature = "std")]
pub use calibration_json::{
    get_intrinsics_from_opencv_camera_calibration_json,
    get_isometry_from_opencv_stereo_calibration_json, write_opencv_minimal_camera_calibration_json,
    write_opencv_minimal_stereo_calibration_json,
};
#[cfg(feature = "std")]
pub use intrinsics_convert::ros_opencv_intrinsics_type_convert;
pub use plane::Plane;
pub use screen_calibration::{
    MARKER_PATTERN_LEN, MAX_SCREEN_ID, ScreenCalibration, ScreenCalibrationError,
};
#[cfg(feature = "std")]
pub use screen_surface::{
    MAX_GRID_DIMENSION, MAX_GRID_VERTICES, ScreenSurface, ScreenSurfaceError, SurfaceHit,
    triangle_hit,
};
