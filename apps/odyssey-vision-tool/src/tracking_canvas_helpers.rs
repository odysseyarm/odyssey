use std::f64::consts::PI;
use std::sync::Arc;

use arrayvec::ArrayVec;
use eframe::egui::{self, Color32, Painter, Rect, Stroke};
use nalgebra::{Isometry3, Point2, Rotation2, Scale2, Transform2, Translation2, Vector2, Vector3};
use parking_lot::Mutex;

use crate::MotState;
use crate::custom_shapes::{
    self, draw_crosshair_rotated, draw_diamond, draw_rect, draw_square, draw_text, to_pos,
};
use crate::mot_runner::MotRunner;

/// Canvas background; dark so it blends with the egui dark theme.
const BACKGROUND: Color32 = Color32::from_gray(20);
/// Neutral line colour for outlines that were black on the light iui canvas.
const OUTLINE: Color32 = Color32::from_gray(200);
/// Half-transparent red fill (premultiplied) for near field boxes.
const NF_FILL: Color32 = Color32::from_rgba_premultiplied(128, 0, 0, 128);
/// Half-transparent blue fill (premultiplied) for wide field boxes.
const WF_FILL: Color32 = Color32::from_rgba_premultiplied(0, 0, 128, 128);

/// Which marker source the canvas visualises.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Unprocessed sensor blobs with their bounding boxes.
    Raw,
    /// Calibrated markers and reprojections.
    Processed,
}

/// Fills the available space of `ui` with the tracking visualisation.
pub(crate) fn show(ui: &mut egui::Ui, runner: &Arc<Mutex<MotRunner>>, source: Source) {
    let size = ui.available_size();
    let (response, painter) = ui.allocate_painter(size, egui::Sense::hover());
    painter.rect_filled(response.rect, 0.0, BACKGROUND);
    draw(&painter, response.rect, runner, source);
}

fn draw(painter: &Painter, rect: Rect, runner: &Arc<Mutex<MotRunner>>, source: Source) {
    let awidth = f64::from(rect.width());
    let aheight = f64::from(rect.height());
    let center = rect.center();
    let draw_size = (awidth.min(aheight).powi(2) / 2.0).sqrt();
    let stroke1 = Stroke::new(1., OUTLINE);
    let stroke2 = Stroke::new(2., OUTLINE);
    let painter = &painter.with_clip_rect(rect);

    let runner = runner.lock();
    let state = &runner.state;

    let gravity_vec = state.orientation.inverse_transform_vector(&Vector3::z());
    let gravity_angle = f64::atan2(f64::from(-gravity_vec.z), f64::from(-gravity_vec.x)) + PI / 2.;

    let center_tf = Translation2::new(f64::from(center.x), f64::from(center.y)).to_homogeneous();

    // Border around the drawing area
    // For POC markers, use 4:3 aspect ratio (matching PAG7665QN 320x240 sensor)
    {
        let border_transform = Transform2::from_matrix_unchecked(
            center_tf
                * Rotation2::new(-gravity_angle).to_homogeneous()
                * Scale2::new(draw_size, draw_size).to_homogeneous(),
        );
        if state.is_poc_markers {
            // Draw 4:3 aspect ratio box for POC markers (matching 320x240 sensor)
            draw_rect(painter, 0.5, 0.375, border_transform, stroke1);
        } else {
            draw_square(painter, border_transform, stroke1);
        }
    }

    // Green line representing the up direction relative to the vision module.
    {
        let angle = -gravity_angle - PI / 2.;
        let tip = to_pos(Point2::new(
            f64::from(center.x) + 50.0 * angle.cos(),
            f64::from(center.y) + 50.0 * angle.sin(),
        ));
        painter.line_segment([center, tip], Stroke::new(2., Color32::from_rgb(0, 255, 0)));
    }

    let draw_tf = Transform2::from_matrix_unchecked(
        center_tf * Scale2::new(draw_size, draw_size).to_homogeneous(),
    );

    draw_text(
        painter,
        rect.min + egui::vec2(20.0, 20.0),
        &format!("screen_id = {}", state.fv_state.screen_id),
    );

    let gravity_rot = Rotation2::new(-gravity_angle);

    draw_text(
        painter,
        rect.min + egui::vec2(20.0, 40.0),
        &format!("gravity_angle = {:.3}", gravity_angle.to_degrees()),
    );

    match source {
        Source::Raw => draw_raw(painter, state, draw_tf, gravity_rot, stroke2),
        Source::Processed => {
            let screen_calibration = runner
                .screen_calibrations
                .iter()
                .find(|x| x.0 == state.fv_state.screen_id);
            draw_not_raw(
                painter,
                state,
                &runner.general_config,
                draw_tf,
                gravity_rot,
                screen_calibration,
            );
        }
    }

    // Center point
    draw_diamond(painter, center, 8.0, 8.0, stroke2);
}

fn draw_raw(
    painter: &Painter,
    state: &MotState,
    draw_tf: Transform2<f64>,
    gravity_rot: Rotation2<f64>,
    marker_stroke: Stroke,
) {
    if let Some(nf_data) = state.nf_data.as_ref() {
        let mut nf_points = ArrayVec::<Point2<f64>, 16>::new();
        for (i, mot_data) in nf_data.iter().enumerate() {
            if mot_data.area == 0 {
                continue;
            }
            // todo don't use hardcoded 4095x4095 res assumption
            let p = Point2::new(mot_data.cx, mot_data.cy).cast::<f64>() / 4095.
                - Vector2::new(0.5, 0.5);
            let p = gravity_rot * p;
            nf_points.push((p + Vector2::new(0.5, 0.5)) * 4095.);
            let p = draw_tf * p;

            custom_shapes::fill_rectangle(
                painter,
                &boundary_bounds(mot_data),
                &gravity_rot,
                &draw_tf,
                NF_FILL,
            );
            custom_shapes::draw_marker(
                painter,
                to_pos(p),
                &format!(
                    "({:.3}, {:.3}) id={} area={}",
                    mot_data.cx, mot_data.cy, i, mot_data.area
                ),
                marker_stroke,
            );
        }
    }

    if let Some(wf_data) = state.wf_data.as_ref() {
        for (i, mot_data) in wf_data.iter().enumerate() {
            if mot_data.area == 0 {
                continue;
            }

            let p = Point2::new(mot_data.cx, mot_data.cy).cast::<f64>() / 4095.
                - Vector2::new(0.5, 0.5);
            let p = gravity_rot * p;
            let p = draw_tf * p;

            custom_shapes::fill_rectangle(
                painter,
                &boundary_bounds(mot_data),
                &gravity_rot,
                &draw_tf,
                WF_FILL,
            );
            custom_shapes::draw_marker(
                painter,
                to_pos(p),
                &format!(
                    "({:.3}, {:.3}) id={} area={}",
                    mot_data.cx, mot_data.cy, i, mot_data.area
                ),
                marker_stroke,
            );
        }
    }
}

/// Bounding box of a blob as `[left, down, right, up]` in unit sensor coordinates.
fn boundary_bounds(mot_data: &odyssey_usb::packets::vm::MotData) -> [f64; 4] {
    [
        f64::from(mot_data.boundary_left) / 98.,
        f64::from(mot_data.boundary_down) / 98.,
        f64::from(mot_data.boundary_right) / 98.,
        f64::from(mot_data.boundary_up) / 98.,
    ]
}

/// Colour for a marker by its pattern id; ids below `known` cycle red/green/blue, others are magenta.
fn pattern_color(pattern_id: Option<u8>, known: u8, unmatched: Color32) -> Color32 {
    match pattern_id {
        Some(id) if id < known => match id % 3 {
            0 => Color32::from_rgb(255, 0, 0),
            1 => Color32::from_rgb(0, 255, 0),
            _ => Color32::from_rgb(0, 0, 255),
        },
        Some(_) => Color32::from_rgb(255, 0, 255),
        None => unmatched,
    }
}

fn draw_not_raw(
    painter: &Painter,
    state: &MotState,
    config: &odyssey_usb::device::GeneralSettings,
    draw_tf: Transform2<f64>,
    gravity_rot: Rotation2<f64>,
    screen: Option<&(u8, odyssey_common::ScreenCalibration<f32>)>,
) {
    let fx = config.camera_model_nf.p.m11;
    let fy = config.camera_model_nf.p.m22;

    // After to_normalized_image_coordinates, coords are in normalized image space.
    // Scale to fit display box. The divisor matches the sensor's max coordinate range.
    // For POC: box is 0.5 x 0.375 (4:3 aspect), sensor is 20479 x 15359
    // For regular: box is 0.5 x 0.5 (square), sensor is 4095 x 4095
    let normalized_scale = if state.is_poc_markers {
        // POC: scale so edge of sensor maps to edge of box
        Scale2::new(fx / 20479.0, fy / 15359.0)
    } else {
        // Regular: 4095x4095 max coordinate range, square box
        Scale2::new(fx / 4095.0, fy / 4095.0)
    };

    if state.is_poc_markers {
        tracing::debug!(
            "POC scale: fx={}, fy={}, scale_x={}, scale_y={}",
            fx,
            fy,
            fx / 20479.0,
            fy / 15359.0
        );
    }

    let thin = |color| Stroke::new(1., color);

    for marker in &state.nf_markers2 {
        let p = normalized_scale * marker.normalized;
        if state.is_poc_markers {
            tracing::debug!(
                "POC marker: raw=({}, {}), scaled=({}, {}), fx={}, fy={}",
                marker.normalized.x,
                marker.normalized.y,
                p.x,
                p.y,
                fx,
                fy
            );
        }
        let p = gravity_rot * p.cast();
        let p = draw_tf * p;

        custom_shapes::draw_marker(
            painter,
            to_pos(p),
            &format!(
                "nf: x={:.2} y={:.2}",
                marker.normalized.x, marker.normalized.y
            ),
            thin(pattern_color(marker.pattern_id, 9, OUTLINE)),
        );
    }

    for marker in &state.wf_markers2 {
        let p = normalized_scale * marker.normalized;
        let p = gravity_rot * p.cast();
        let p = draw_tf * p;

        custom_shapes::draw_marker_rotated(
            painter,
            to_pos(p),
            "wf",
            thin(pattern_color(
                marker.pattern_id,
                6,
                Color32::from_rgb(255, 0, 0),
            )),
        );
    }

    // Projects a camera-space point into the canvas through the near field intrinsics.
    let project = |p: nalgebra::Point3<f32>| {
        let p = p / p.z;
        // todo don't use hardcoded 4095x4095 res assumption
        let p = Point2::new(p.x * fx, p.y * fy) / 4095.;
        to_pos(draw_tf * (gravity_rot * p.cast()))
    };

    if let Some((_, screen_calibration)) = screen {
        for p in screen_calibration.object_points {
            // eskf reprojections
            let position = state.fv_state.filter.position;
            let orientation = state.fv_state.filter.orientation;
            let reproj_tf = Isometry3::from_parts(position.into(), orientation);
            let p = project(reproj_tf.cast().inverse_transform_point(&p));
            draw_crosshair_rotated(painter, p, 20., thin(Color32::from_rgb(0, 176, 107)));
        }
    }
    let pnp_iso = odyssey_cv::telemetry::pnp_solutions().get_last();
    if let Some(pnp_iso) = pnp_iso
        && let Some((_, screen_calibration)) = screen
    {
        for p in screen_calibration.object_points {
            // pnp reprojections
            let reproj_tf = pnp_iso.inverse();
            let p = project(reproj_tf.cast().inverse_transform_point(&p));
            draw_crosshair_rotated(painter, p, 20., thin(Color32::from_gray(77)));
        }
    }
    for p in &state.wf_reproj {
        // todo don't use hardcoded 4095x4095 res assumption
        let p = Point2::new(p.x * fx, p.y * fy) / 4095.;
        let p = to_pos(draw_tf * (gravity_rot * p.cast()));
        draw_crosshair_rotated(
            painter,
            p,
            20.,
            Stroke::new(2., Color32::from_rgb(160, 32, 240)),
        );
    }
}
