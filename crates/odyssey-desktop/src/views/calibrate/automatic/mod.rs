//! Automatic calibration: a depth camera measures the screen and the tracker
//! markers while this app shows a dot pattern, and the result is saved as a
//! screen calibration.
//!
//! [`open`] starts the guided task in the main workspace. Only the measurement
//! pattern opens on the selected display; the workspace retains its selections.

mod attempt_bundle;
mod attempt_button;
mod calibrate_automatic;
mod camera_support;
pub(crate) mod hints;
mod illustrations;
pub(crate) mod kind;
mod markers;
mod measuring;
pub(crate) mod pattern_window;
mod position;
mod problem;
pub(crate) mod quality;
pub(crate) mod reference;
mod reference_helper;
mod review;
mod scale_board_info;
pub(crate) mod screens;
mod sdk_download;
pub(crate) mod settings;
mod setup;
mod ui;
pub(crate) mod wizard;
pub(crate) mod worker;

pub(crate) use calibrate_automatic::CalibrateAutomaticView;
use gpui::App;

use crate::cli::CalibrateArgs;

pub(crate) fn open(args: Option<CalibrateArgs>, cx: &mut App) {
    crate::shell::show_calibration(args, cx);
}
