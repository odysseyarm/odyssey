use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Shape, Stroke, Vec2};
use nalgebra::{Matrix3, Point2, Rotation2, Transform2, Vector2};

/// Colour of on-canvas labels; readable on the dark canvas background.
pub(crate) const TEXT_COLOR: Color32 = Color32::from_gray(220);

/// Converts a canvas-space point (screen pixels, y down) to an egui position.
pub(crate) fn to_pos(p: Point2<f64>) -> Pos2 {
    let p = p.cast::<f32>();
    Pos2::new(p.x, p.y)
}

pub(crate) fn draw_crosshair(painter: &Painter, center: Pos2, r: f32, stroke: Stroke) {
    painter.line_segment(
        [
            Pos2::new(center.x - r, center.y),
            Pos2::new(center.x + r, center.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(center.x, center.y - r),
            Pos2::new(center.x, center.y + r),
        ],
        stroke,
    );
}

pub(crate) fn draw_crosshair_rotated(painter: &Painter, center: Pos2, r: f32, stroke: Stroke) {
    let r = r * std::f32::consts::FRAC_1_SQRT_2;
    painter.line_segment(
        [
            Pos2::new(center.x - r, center.y - r),
            Pos2::new(center.x + r, center.y + r),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(center.x - r, center.y + r),
            Pos2::new(center.x + r, center.y - r),
        ],
        stroke,
    );
}

/// Draws a `x_subdiv` by `y_subdiv` grid over the unit square mapped through `transform`,
/// then shifted by `offset`.
pub(crate) fn draw_grid(
    painter: &Painter,
    x_subdiv: usize,
    y_subdiv: usize,
    transform: Matrix3<f64>,
    offset: Vec2,
    stroke: Stroke,
) {
    let map = |x: f64, y: f64| to_pos(transform.transform_point(&Point2::new(x, y))) + offset;
    for y in 0..=y_subdiv {
        let v = y as f64 / y_subdiv as f64;
        painter.line_segment([map(0.0, v), map(1.0, v)], stroke);
    }
    for x in 0..=x_subdiv {
        let v = x as f64 / x_subdiv as f64;
        painter.line_segment([map(v, 0.0), map(v, 1.0)], stroke);
    }
}

pub(crate) fn draw_diamond(painter: &Painter, center: Pos2, w: f32, h: f32, stroke: Stroke) {
    painter.add(Shape::closed_line(
        vec![
            Pos2::new(center.x, center.y - h / 2.),
            Pos2::new(center.x + w / 2., center.y),
            Pos2::new(center.x, center.y + h / 2.),
            Pos2::new(center.x - w / 2., center.y),
        ],
        stroke,
    ));
}

/// Outlines a square spanning \[-0.5, 0.5\] on both axes, mapped through `transform`.
pub(crate) fn draw_square(painter: &Painter, transform: Transform2<f64>, stroke: Stroke) {
    draw_rect(painter, 0.5, 0.5, transform, stroke);
}

/// Outlines a rectangle with the given half extents centered at the origin, mapped through
/// `transform`. For a 4:3 aspect ratio POC box: `half_width = 0.5`, `half_height = 0.375`.
pub(crate) fn draw_rect(
    painter: &Painter,
    half_width: f64,
    half_height: f64,
    transform: Transform2<f64>,
    stroke: Stroke,
) {
    let corners = [
        Point2::new(-half_width, -half_height),
        Point2::new(half_width, -half_height),
        Point2::new(half_width, half_height),
        Point2::new(-half_width, half_height),
    ];
    painter.add(Shape::closed_line(
        corners.iter().map(|c| to_pos(transform * c)).collect(),
        stroke,
    ));
}

/// Draws a crosshair and associated text at a given position.
pub(crate) fn draw_marker(painter: &Painter, position: Pos2, label: &str, stroke: Stroke) {
    draw_crosshair(painter, position, 50.0, stroke);
    draw_text(painter, position + egui::vec2(20.0, 20.0), label);
}

/// Draws a rotated crosshair and associated text at a given position.
pub(crate) fn draw_marker_rotated(painter: &Painter, position: Pos2, label: &str, stroke: Stroke) {
    draw_crosshair_rotated(painter, position, 50.0, stroke);
    draw_text(painter, position + egui::vec2(20.0, 50.0), label);
}

/// Fills a rectangle defined by boundaries (`[left, down, right, up]`) and transforms.
pub(crate) fn fill_rectangle(
    painter: &Painter,
    bounds: &[f64; 4],
    gravity_rot: &Rotation2<f64>,
    draw_tf: &Transform2<f64>,
    fill: Color32,
) {
    let corner = Point2::new(bounds[0] - 0.5, bounds[3] - 0.5);
    let width = bounds[2] - bounds[0];
    let height = bounds[1] - bounds[3];
    let a = gravity_rot * corner;
    let horiz = gravity_rot * Vector2::x() * width;
    let vert = gravity_rot * Vector2::y() * height;
    let points = [
        draw_tf * a,
        draw_tf * (a + horiz),
        draw_tf * (a + horiz + vert),
        draw_tf * (a + vert),
    ];
    painter.add(Shape::convex_polygon(
        points.iter().map(|p| to_pos(*p)).collect(),
        fill,
        Stroke::NONE,
    ));
}

/// Draws monospace text with its top-left corner at `pos`.
pub(crate) fn draw_text(painter: &Painter, pos: Pos2, s: &str) {
    painter.text(
        pos,
        Align2::LEFT_TOP,
        s,
        FontId::monospace(12.0),
        TEXT_COLOR,
    );
}
