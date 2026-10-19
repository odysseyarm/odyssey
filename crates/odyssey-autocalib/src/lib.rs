//! Automatic screen calibration from a depth camera.
//!
//! A camera looks at the screen while the screen shows a known dot pattern.
//! The depth image gives the screen's plane in metric units, the color image
//! gives the pattern's pose within that plane, and the infrared image gives the
//! positions of the tracker markers. [`solve`] turns those into the
//! [`odyssey_common::ScreenCalibration`] the hub consumes.
//!
//! Camera access goes through [`camera::CameraBackend`], so the algorithm runs
//! against the [`synthetic`] backend in tests and against real hardware through
//! [`orbbec`], which loads the Orbbec SDK at run time and works without it
//! being installed until a camera is actually needed.

pub mod board;
pub mod camera;
pub mod display;
pub mod imu;
pub mod markers;
pub mod orbbec;
pub mod pattern;
pub mod placement;
pub mod plane;
pub mod session;
pub mod solve;
pub mod synthetic;
