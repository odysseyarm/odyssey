//! Automatic screen calibration from a depth camera.
//!
//! A camera looks at the screen while the screen shows a known dot pattern.
//!
//! The depth image gives the screen's plane in metric units, the color image
//! gives the pattern's pose within that plane, and the infrared image gives the
//! positions of the tracker markers. [`solve`] turns those into the
//! [`odyssey_common::ScreenCalibration`] the hub consumes.
//!
//! Camera access goes through [`camera::CameraBackend`], so the algorithm runs
//! against the [`synthetic`] backend in tests and against real hardware through
//! [`orbbec`], which loads the Orbbec SDK at run time and works without it
//! being installed until a camera is actually needed.

/// Since: Odyssey Desktop 0.6.7.
pub mod board;
/// Since: Odyssey Desktop 0.6.5.
pub mod camera;
/// Since: Odyssey Desktop 0.6.5.
pub mod display;
/// Since: Odyssey Desktop 0.6.5.
pub mod imu;
/// Since: Odyssey Desktop 0.6.5.
pub mod markers;
/// Since: Odyssey Desktop 0.6.5.
#[expect(
    unsafe_code,
    reason = "Orbbec SDK FFI: the bindgen bindings and the run-time loader"
)]
pub mod orbbec;
/// Since: Odyssey Desktop 0.6.5.
pub mod pattern;
/// Since: Odyssey Desktop 0.6.7.
pub mod placement;
/// Since: Odyssey Desktop 0.6.5.
pub mod plane;
/// Since: Odyssey Desktop 0.6.5.
pub mod session;
/// Since: Odyssey Desktop 0.6.5.
pub mod solve;
/// Since: Odyssey Desktop 0.6.5.
pub mod synthetic;
