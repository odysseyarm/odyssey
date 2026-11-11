use std::sync::Arc;

use eframe::egui;
use parking_lot::Mutex;

use crate::mot_runner::MotRunner;
use crate::tracking_canvas_helpers::{self, Source};

/// Tracking view with unprocessed markers, filling the available space.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn show(ui: &mut egui::Ui, runner: &Arc<Mutex<MotRunner>>) {
    tracking_canvas_helpers::show(ui, runner, Source::Raw);
}
