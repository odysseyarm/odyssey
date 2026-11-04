//! A "where are my screens" picker: the machine's monitors drawn to scale in
//! the arrangement the OS uses, one clickable tile per monitor.
//!
//! [`use_monitor_selection`] owns the monitor list and the remembered choice;
//! [`ScreenPicker`] draws it. Any view that needs "which monitor" shares one
//! [`MonitorSelection`] with the picker.

mod identify_overlay;
mod monitor_layout;
mod monitor_selection;
mod picker;

pub(crate) use monitor_layout::monitor_key;
pub(crate) use monitor_selection::{MonitorSelection, use_monitor_selection};
pub(crate) use picker::ScreenPicker;
