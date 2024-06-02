use ggez::{glam::{vec2, vec3, vec4, Mat4, Vec4Swizzles}, graphics::{Canvas, Color, DrawMode, DrawParam, Mesh, Transform}, Context};
use opencv::core::Point2f;

pub struct DrawPatternPoints {
    circle: ggez::graphics::Mesh,
}

impl DrawPatternPoints {
    pub fn new(ctx: &mut Context) -> Self {
        Self {
            circle: Mesh::new_circle(ctx, DrawMode::stroke(1.0), vec2(0., 0.), 6.0, 1.0, Color::WHITE).unwrap(),
        }
    }

    pub fn draw(&self, ctx: &mut Context, canvas: &mut Canvas, points: &[Point2f], cols: usize, pattern_was_found: bool, transform: Transform) {
        let transform = Mat4::from(transform.to_bare_matrix());
        let colors = [
            (255, 0, 0),
            (255,128,0),
            (200,200,0),
            (0,255,0),
            (0,200,200),
            (0,0,255),
            (255,0,255)
        ];
        for (i, point) in points.iter().enumerate() {
            if pattern_was_found {
                canvas.draw(
                    &self.circle,
                    DrawParam::default()
                        .dest((transform * vec4(point.x, point.y, 0., 1.)).xy())
                        .color(colors[i / cols % colors.len()])
                );
            } else {
                canvas.draw(
                    &self.circle,
                    DrawParam::default()
                        .dest((transform * vec4(point.x, point.y, 0., 1.)).xy())
                        .color(colors[0])
                );
            }
        }
    }
}
