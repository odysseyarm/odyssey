//! Device inspection through discovered USB and dongle links.
//!
//! The main executable accepts no device-address arguments. Legacy simulator
//! and M4Hub picker entries had no transport implementation and are removed;
//! simulation is an opt-in host transport, not a network address in this picker.

pub mod app;
#[cfg(feature = "bevy")]
pub mod bevy_viewer;
pub mod config_window;
pub mod consts;
mod imaging;
pub mod mot_runner;
mod mot_state;
pub mod plots_window;
pub mod recording;
pub mod run_canvas;
pub mod run_raw_canvas;
mod screen_info;
pub mod test_canvas;
mod test_frame;
pub mod theme;

mod custom_shapes;
mod tracking_canvas_helpers;

pub use mot_state::{Marker, MotState};
pub use screen_info::ScreenInfo;
pub use test_frame::TestFrame;
