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
        transform: Mat4,
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

        for (i, point) in points.iter().enumerate() {
            let point = vec3(
                point.x as f32 / 4094. * 97. + 0.5,
                point.y as f32 / 4094. * 97. + 0.5,
                0.0,
            );
            let transformed_point = transform.transform_point3(point);
            let color = if pattern_was_found {
                colors[i / cols % colors.len()]
            } else {
                Color::from_rgba(0, 0, 255, 255)
            };

            draw_circle_lines(
                transformed_point.x,
                transformed_point.y,
                6.0,
                1.0,
                color,
            );

            // Draw the line between points
            if pattern_was_found && i > 0 {
                let prev_point = vec3(
                    points[i-1].x as f32 / 4094. * 97. + 0.5,
                    points[i-1].y as f32 / 4094. * 97. + 0.5,
                    0.0,
                );
                let prev_transformed_point = transform.transform_point3(prev_point);
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

// Define a function to create the transformation matrix
pub fn get_transform_matrix(scale: Vec2, rotation: f32, dest: Vec2, offset: Vec2) -> Mat4 {
    // Create a scaling matrix
    let scale_matrix = Mat4::from_scale(Vec3::new(scale.x, scale.y, 1.0));

    // Create a rotation matrix (rotation in radians)
    let rotation_matrix = Mat4::from_rotation_z(rotation);

    // Create a translation matrix with destination offset
    let translation_matrix = Mat4::from_translation(dest.extend(0.0));

    // Create a translation matrix for offset
    let offset_matrix = Mat4::from_translation(-offset.extend(0.0));

    // Combine the transformations: translation * rotation * scale * offset
    translation_matrix * rotation_matrix * scale_matrix * offset_matrix
}
