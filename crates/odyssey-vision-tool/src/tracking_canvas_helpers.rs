use std::f64::consts::PI;
use std::sync::Arc;

use arrayvec::ArrayVec;
use eframe::egui::{self, Color32, Painter, Rect, Stroke};
use nalgebra::{Isometry3, Point2, Rotation2, Scale2, Transform2, Translation2, Vector2, Vector3};
use parking_lot::Mutex;

use crate::Marker;
use crate::custom_shapes::{
    self, draw_crosshair_rotated, draw_diamond, draw_rect, draw_square, draw_text, to_pos,
};
use crate::mot_runner::MotRunner;

/// Canvas background, dark so it blends with the egui dark theme.
const BACKGROUND: Color32 = Color32::from_gray(20);
/// Neutral line colour for outlines, light enough to read on [`BACKGROUND`].
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

/// Copies only display data under the acquisition lock. Text layout and paint
/// generation must not hold up device packets at the display's refresh rate.
struct CanvasState {
    gravity_angle: f64,
    screen_id: u8,
    legacy_markers: bool,
    nf_data: Option<ArrayVec<odyssey_usb::packets::vm::legacy::MotData, 16>>,
    wf_data: Option<ArrayVec<odyssey_usb::packets::vm::legacy::MotData, 16>>,
    nf_markers2: ArrayVec<Marker, 16>,
    wf_markers2: ArrayVec<Marker, 16>,
    wf_reproj: ArrayVec<Point2<f32>, 16>,
    focal: Vector2<f32>,
    pose: Isometry3<f32>,
    screen: Option<odyssey_common::ScreenCalibration<f32>>,
}

impl CanvasState {
    fn snapshot(runner: &MotRunner, source: Source) -> Self {
        let state = &runner.state;
        let gravity = state.orientation.inverse_transform_vector(&Vector3::z());
        let raw = source == Source::Raw;
        let processed = source == Source::Processed;
        Self {
            gravity_angle: f64::atan2(f64::from(-gravity.z), f64::from(-gravity.x)) + PI / 2.,
            screen_id: state.fv_state.screen_id,
            legacy_markers: state.legacy_markers,
            nf_data: if raw { state.nf_data.clone() } else { None },
            wf_data: if raw { state.wf_data.clone() } else { None },
            nf_markers2: if processed {
                state.nf_markers2.clone()
            } else {
                ArrayVec::new()
            },
            wf_markers2: if processed {
                state.wf_markers2.clone()
            } else {
                ArrayVec::new()
            },
            wf_reproj: if processed {
                state.wf_reproj.clone()
            } else {
                ArrayVec::new()
            },
            focal: Vector2::new(
                runner.general_config.camera_model_nf.p.m11,
                runner.general_config.camera_model_nf.p.m22,
            ),
            pose: Isometry3::from_parts(
                state.fv_state.filter.position.into(),
                state.fv_state.filter.orientation,
            ),
            screen: if processed {
                runner
                    .screen_calibrations
                    .iter()
                    .find(|(id, _)| *id == state.fv_state.screen_id)
                    .map(|(_, calibration)| calibration.clone())
            } else {
                None
            },
        }
    }
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

    let state = CanvasState::snapshot(&runner.lock(), source);
    let gravity_angle = state.gravity_angle;

    let center_tf = Translation2::new(f64::from(center.x), f64::from(center.y)).to_homogeneous();

    // Current reports come from the 320x240 PAG7665QN, so their border is 4:3
    // instead of the legacy square.
    {
        let border_transform = Transform2::from_matrix_unchecked(
            center_tf
                * Rotation2::new(-gravity_angle).to_homogeneous()
                * Scale2::new(draw_size, draw_size).to_homogeneous(),
        );
        if state.legacy_markers {
            draw_square(painter, border_transform, stroke1);
        } else {
            draw_rect(painter, 0.5, 0.375, border_transform, stroke1);
        }
    }

    // Green line representing the up direction relative to the vision module.
    let angle = -gravity_angle - PI / 2.;
    let tip = to_pos(Point2::new(
        f64::from(center.x) + 50.0 * angle.cos(),
        f64::from(center.y) + 50.0 * angle.sin(),
    ));
    painter.line_segment([center, tip], Stroke::new(2., Color32::from_rgb(0, 255, 0)));

    let draw_tf = Transform2::from_matrix_unchecked(
        center_tf * Scale2::new(draw_size, draw_size).to_homogeneous(),
    );

    draw_text(
        painter,
        rect.min + egui::vec2(20.0, 20.0),
        &format!("screen_id = {}", state.screen_id),
    );

    let gravity_rot = Rotation2::new(-gravity_angle);

    draw_text(
        painter,
        rect.min + egui::vec2(20.0, 40.0),
        &format!("gravity_angle = {:.3}", gravity_angle.to_degrees()),
    );

    match source {
        Source::Raw => draw_raw(painter, &state, draw_tf, gravity_rot, stroke2),
        Source::Processed => draw_not_raw(painter, &state, draw_tf, gravity_rot),
    }

    draw_diamond(painter, center, 8.0, 8.0, stroke2);
}

fn draw_raw(
    painter: &Painter,
    state: &CanvasState,
    draw_tf: Transform2<f64>,
    gravity_rot: Rotation2<f64>,
    marker_stroke: Stroke,
) {
    if let Some(nf_data) = state.nf_data.as_ref() {
        for (i, mot_data) in nf_data.iter().enumerate() {
            if mot_data.area == 0 {
                continue;
            }
            // TODO: assumes a 4095x4095 sensor; take the resolution from the device.
            let p = Point2::new(mot_data.cx, mot_data.cy).cast::<f64>() / 4095.
                - Vector2::new(0.5, 0.5);
            let p = gravity_rot * p;
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
fn boundary_bounds(mot_data: &odyssey_usb::packets::vm::legacy::MotData) -> [f64; 4] {
    [
        f64::from(mot_data.boundary_left) / 98.,
        f64::from(mot_data.boundary_down) / 98.,
        f64::from(mot_data.boundary_right) / 98.,
        f64::from(mot_data.boundary_up) / 98.,
    ]
}

/// Colour for a marker by its pattern id. Ids below `known` cycle red/green/blue,
/// others are magenta.
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
    state: &CanvasState,
    draw_tf: Transform2<f64>,
    gravity_rot: Rotation2<f64>,
) {
    // WF rays have already been rotated into the NF camera's coordinates, so
    // both use the NF focal length. The current 4:3 box keeps the same scale on
    // each axis: 15359 / 20479 is its displayed 0.75 height-to-width ratio.
    let sensor_width = if state.legacy_markers {
        4095.0
    } else {
        20479.0
    };
    let normalized_scale = Scale2::new(state.focal.x / sensor_width, state.focal.y / sensor_width);

    let thin = |color| Stroke::new(1., color);

    for marker in &state.nf_markers2 {
        let p = normalized_scale * marker.normalized;
        if !p.coords.iter().all(|coordinate| coordinate.is_finite()) {
            continue;
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
        if !p.coords.iter().all(|coordinate| coordinate.is_finite()) {
            continue;
        }
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
    let project = |point: nalgebra::Point3<f32>| {
        if point.z <= 0.0 || !point.coords.iter().all(|coordinate| coordinate.is_finite()) {
            return None;
        }
        let point = normalized_scale * Point2::new(point.x / point.z, point.y / point.z);
        Some(to_pos(draw_tf * (gravity_rot * point.cast())))
    };

    if let Some(screen_calibration) = &state.screen {
        for point in screen_calibration.object_points {
            let Some(p) = project(state.pose.cast().inverse_transform_point(&point)) else {
                continue;
            };
            draw_crosshair_rotated(painter, p, 20., thin(Color32::from_rgb(0, 176, 107)));
        }
    }
    let pnp_iso = odyssey_cv::telemetry::pnp_solutions().get_last();
    if let Some(pnp_iso) = pnp_iso
        && let Some(screen_calibration) = &state.screen
    {
        for point in screen_calibration.object_points {
            let Some(p) = project(pnp_iso.cast().transform_point(&point)) else {
                continue;
            };
            draw_crosshair_rotated(painter, p, 20., thin(Color32::from_gray(77)));
        }
    }
    for p in &state.wf_reproj {
        let p = normalized_scale * p;
        if !p.coords.iter().all(|coordinate| coordinate.is_finite()) {
            continue;
        }
        let p = to_pos(draw_tf * (gravity_rot * p.cast()));
        draw_crosshair_rotated(
            painter,
            p,
            20.,
            Stroke::new(2., Color32::from_rgb(160, 32, 240)),
        );
    }
}
