use macroquad::prelude::*;
use opencv::core::Point2f;

pub struct DrawPatternPoints;

impl DrawPatternPoints {
    pub fn new() -> Self {
        Self {}
    }

    pub fn draw(
        &self,
        points: &[Point2f],
        cols: usize,
        pattern_was_found: bool,
        resolution: (u16, u16),
        object_resolution: (u32, u32),
        transform: Mat3,
    ) {
        let colors = [
            Color::from_rgba(255, 0, 0, 255),
            Color::from_rgba(255, 128, 0, 255),
            Color::from_rgba(200, 200, 0, 255),
            Color::from_rgba(0, 255, 0, 255),
            Color::from_rgba(0, 200, 200, 255),
            Color::from_rgba(0, 0, 255, 255),
            Color::from_rgba(255, 0, 255, 255),
        ];

        let rx = resolution.0 as f32;
        let ry = resolution.1 as f32;
        let ox = object_resolution.0 as f32;
        let oy = object_resolution.1 as f32;
        for (i, point) in points.iter().enumerate() {
            let point = vec2(
                point.x as f32 / (ox - 1.0) * (rx - 1.0) + 0.5,
                point.y as f32 / (oy - 1.0) * (ry - 1.0) + 0.5,
            );
            let transformed_point = transform.transform_point2(point);
            let color = if pattern_was_found {
                colors[i / cols % colors.len()]
            } else {
                Color::from_rgba(255, 0, 0, 255)
            };

            draw_circle_lines(transformed_point.x, transformed_point.y, 6.0, 2.0, color);

            if pattern_was_found && i > 0 {
                let prev_point = vec2(
                    points[i - 1].x as f32 / (ox - 1.0) * (rx - 1.0) + 0.5,
                    points[i - 1].y as f32 / (oy - 1.0) * (ry - 1.0) + 0.5,
                );
                let prev_transformed_point = transform.transform_point2(prev_point);
                draw_line(
                    prev_transformed_point.x,
                    prev_transformed_point.y,
                    transformed_point.x,
                    transformed_point.y,
                    1.0,
                    color,
                )
            }
        }
    }
}

/// Moves `offset` to the origin, scales, rotates by `rotation` radians about z, then translates to `dest`.
pub fn get_transform_matrix(scale: Vec2, rotation: f32, dest: Vec2, offset: Vec2) -> Mat4 {
    let scale_matrix = Mat4::from_scale(Vec3::new(scale.x, scale.y, 1.0));

    let rotation_matrix = Mat4::from_rotation_z(rotation);

    let translation_matrix = Mat4::from_translation(dest.extend(0.0));

    let offset_matrix = Mat4::from_translation(-offset.extend(0.0));

    translation_matrix * rotation_matrix * scale_matrix * offset_matrix
}
