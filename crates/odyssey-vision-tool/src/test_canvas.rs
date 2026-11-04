use std::sync::Arc;

use eframe::egui::{self, Color32, Key, Rect, Stroke};
use nalgebra::{Isometry3, Matrix3, Point2, Translation3};
use parking_lot::Mutex;

use crate::custom_shapes::{draw_crosshair, draw_grid, draw_text};
use crate::mot_runner::MotRunner;

const BACKGROUND: Color32 = Color32::from_gray(24);

/// What the host window should do after a frame of the test canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Exit,
}

/// Aimpoint test view; fills the available space and handles its keyboard shortcuts.
pub fn show(ui: &mut egui::Ui, runner: &Arc<Mutex<MotRunner>>) -> Outcome {
    let outcome = handle_keys(ui, runner);

    let size = ui.available_size();
    let (response, painter) = ui.allocate_painter(size, egui::Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, BACKGROUND);
    draw(&painter, rect, runner);
    outcome
}

fn handle_keys(ui: &egui::Ui, runner: &Arc<Mutex<MotRunner>>) -> Outcome {
    let (exit, zero, reset) = ui.input(|i| {
        (
            i.key_pressed(Key::Escape) || i.key_pressed(Key::Q),
            i.key_released(Key::Z),
            i.key_pressed(Key::Backspace),
        )
    });
    if exit {
        return Outcome::Exit;
    }
    if zero {
        let t = Translation3::new(0., -0.0381, 0.);
        // One guard for read and write: parking_lot mutexes are not reentrant.
        let mut runner = runner.lock();
        let quat = odyssey_cv::helpers::calculate_zero_offset_quat(
            t,
            Point2::new(0.5, 0.5),
            &runner.screen_calibrations,
            &runner.state.fv_state,
        );
        if let Some(quat) = quat {
            runner.state.fv_zero_offset = Isometry3::from_parts(t, quat);
        } else {
            tracing::warn!("Failed to calculate zero offset quat");
        }
    }
    if reset {
        runner.lock().state.fv_zero_offset = Isometry3::default();
    }
    Outcome::Continue
}

fn draw(painter: &egui::Painter, rect: Rect, runner: &Arc<Mutex<MotRunner>>) {
    let width = f64::from(rect.width());
    let height = f64::from(rect.height());
    let origin = rect.min;
    // Canvas-local pixels to screen position.
    let at = |x: f32, y: f32| origin + egui::vec2(x, y);

    let (aimpoint, distance, screen_id) = {
        let runner = runner.lock();
        (
            runner.state.fv_aimpoint,
            runner.state.distance,
            runner.state.fv_state.screen_id,
        )
    };

    draw_crosshair(
        painter,
        at(aimpoint.x * rect.width(), aimpoint.y * rect.height()),
        30.,
        Stroke::new(10., Color32::from_rgb(0, 255, 0)),
    );
    draw_text(
        painter,
        at(20.0, 20.0),
        &format!("distance = {distance:.4}"),
    );
    draw_text(painter, at(20.0, 40.0), &format!("screen_id = {screen_id}"));

    // The grid spans the unit square, so its screen mapping is a plain scale
    // by the pixel size, not a perspective solve.
    draw_grid(
        &painter.with_clip_rect(rect),
        10,
        10,
        Matrix3::new(width, 0.0, 0.0, 0.0, height, 0.0, 0.0, 0.0, 1.0),
        origin.to_vec2(),
        Stroke::new(1., Color32::from_rgb(200, 60, 60)),
    );

    let target = Stroke::new(1., Color32::from_rgb(80, 140, 255));
    let center = rect.center();
    draw_crosshair(painter, center, 25., target);
    painter.circle_stroke(center, 25., target);
}
