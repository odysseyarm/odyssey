//! A camera that needs no hardware: it renders what a depth camera with a
//! color imager, an infrared imager and an IMU would see while looking at a
//! screen that shows the dot pattern.
//!
//! The tests drive [`crate::session::Session`] with it, and a GUI can offer it
//! as a demo camera. The scene is a physical screen of known size at a known
//! pose, a camera with a known tilt, and the six IR markers at their layout
//! positions, so [`SyntheticScene::truth`] gives the calibration the solver
//! should recover.
//!
//! Conventions: the *world* frame is gravity aligned with `x` right, `y` down
//! and `z` forward; the camera starts at its origin. Screen and marker
//! coordinates in [`Truth`] use the physical screen frame: origin at the top
//! left corner, `x` right, `y` down, `z` away from the viewer.
//!
//! The scene description lives in `scene`, its noise-free rendering in `view`
//! and the camera that adds noise and an IMU in `stream`. The rendering is the
//! numeric reference for the end-to-end tests, so the order in which it draws
//! random numbers is part of its behavior.

mod scene;
mod stream;
mod view;

pub use scene::{SIMULATED_MARKERS, SyntheticScene, Truth};
pub use stream::{SyntheticBackend, SyntheticStream};
