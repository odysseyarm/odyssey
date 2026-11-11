#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

#[cfg(feature = "std")]
mod calibration_json;
#[cfg(feature = "std")]
mod intrinsics_convert;
/// Since: Odyssey Desktop 0.6.0.
pub mod ocv_types;
mod plane;
mod screen_calibration;
#[cfg(feature = "std")]
mod screen_surface;

/// Since: Odyssey Desktop 0.6.0.
#[cfg(feature = "std")]
pub use calibration_json::{
    get_intrinsics_from_opencv_camera_calibration_json,
    get_isometry_from_opencv_stereo_calibration_json, write_opencv_minimal_camera_calibration_json,
    write_opencv_minimal_stereo_calibration_json,
};
/// Since: Odyssey Desktop 0.6.0.
#[cfg(feature = "std")]
pub use intrinsics_convert::ros_opencv_intrinsics_type_convert;
/// Since: Odyssey Desktop 0.6.0.
pub use plane::Plane;
/// # Since
/// - `MARKER_PATTERN_LEN`: Odyssey Desktop 0.6.0.
/// - `MAX_SCREEN_ID`: Odyssey Desktop 0.6.0.
/// - `ScreenCalibration`: Odyssey Desktop 0.6.0.
/// - `ScreenCalibrationError`: Odyssey Desktop 0.7.0.
pub use screen_calibration::{
    MARKER_PATTERN_LEN, MAX_SCREEN_ID, ScreenCalibration, ScreenCalibrationError,
};
/// Since: Odyssey Desktop 0.7.0.
#[cfg(feature = "std")]
pub use screen_surface::{
    MAX_GRID_DIMENSION, MAX_GRID_VERTICES, ScreenSurface, ScreenSurfaceError, SurfaceHit,
    triangle_hit,
};
