//! A "where are my screens" picker: the machine's displays drawn to scale in
//! the arrangement the OS uses, one clickable tile per display.
//!
//! [`MonitorSelection`] owns the display list and the remembered choice;
//! [`ScreenPicker`] draws it. Any view that needs "which display" shares one
//! `Entity<MonitorSelection>` with the picker.

mod identify_overlay;
mod monitor_layout;
mod monitor_selection;
mod picker;

pub(crate) use monitor_selection::{MonitorSelection, display_for_key, fullscreen_options};
pub(crate) use picker::ScreenPicker;
