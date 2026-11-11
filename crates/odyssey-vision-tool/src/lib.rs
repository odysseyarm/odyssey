//! Device inspection through discovered USB and dongle links.
//!
//! The main executable accepts no device-address arguments. `Legacy` simulator
//! and `M4Hub` picker entries had no transport implementation and are removed.
//! Simulation is an opt-in host transport, not a network address in this picker.

/// Since: Odyssey Desktop 0.7.0.
pub mod app;
/// Since: Odyssey Desktop 0.7.0.
#[cfg(feature = "bevy")]
pub mod bevy_viewer;
/// Since: Odyssey Desktop 0.6.0.
pub mod config_window;
/// Since: Odyssey Desktop 0.6.0.
pub mod consts;
mod imaging;
/// Since: Odyssey Desktop 0.6.0.
pub mod mot_runner;
mod mot_state;
/// Since: Odyssey Desktop 0.6.0.
pub mod plots_window;
/// Since: Odyssey Desktop 0.7.0.
pub mod recording;
/// Since: Odyssey Desktop 0.6.0.
pub mod run_canvas;
/// Since: Odyssey Desktop 0.6.0.
pub mod run_raw_canvas;
mod screen_info;
/// Since: Odyssey Desktop 0.6.0.
pub mod test_canvas;
mod test_frame;
/// Since: Odyssey Desktop 0.7.0.
pub mod theme;

mod custom_shapes;
mod tracking_canvas_helpers;

/// Since: Odyssey Desktop 0.6.0.
pub use mot_state::{Marker, MotState};
/// Since: Odyssey Desktop 0.6.0.
pub use screen_info::ScreenInfo;
/// Since: Odyssey Desktop 0.6.0.
pub use test_frame::TestFrame;
