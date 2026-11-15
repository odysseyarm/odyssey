//! The scene as the camera sees it, free of per-frame noise: the color image
//! of the room and screen, the ideal depth map and where the markers land in
//! the infrared image. A [`crate::synthetic::SyntheticStream`] renders these
//! once and adds noise to a copy for every frame.

use std::ops::Range;

use image::{Rgb, RgbImage};
use nalgebra::{Point2, Vector2, Vector3};
use rand::Rng as _;
use rand::rngs::StdRng;

use super::scene::{Pose, SIMULATED_MARKERS, SyntheticScene};
use crate::camera::{CameraGeometry, CaptureSettings, Extrinsics};
use crate::plane::{Plane, pixel_ray, project};

/// The wall behind the screen is this far behind the screen's center, along
/// the world z axis.
const WALL_BEHIND_SCREEN_M: f64 = 1.5;

/// Color exposure (microseconds, at gain 1) at which scene levels are
/// rendered unscaled.
const REFERENCE_EXPOSURE_US: f64 = 1500.0;

/// The room wall is a gradient `BASE + ACROSS * x / width + DOWN * y / height`
/// in 8-bit levels before exposure scaling.
const WALL_LEVEL_BASE: f64 = 60.0;
const WALL_LEVEL_ACROSS: f64 = 40.0;
const WALL_LEVEL_DOWN: f64 = 15.0;

/// Level of a dark pixel on the panel, which is a little brighter than
/// nothing.
const PANEL_BLACK_LEVEL: f64 = 5.0;

/// Levels above [`PANEL_BLACK_LEVEL`] that a fully covered pixel adds.
const PANEL_LIT_LEVEL_RANGE: f64 = 230.0;

/// Straight edges of the panel are sampled this many times before being
/// projected, so the lens distortion bends them in the outline.
const OUTLINE_STEPS_PER_EDGE: u32 = 48;

/// Dots are painted out to this multiple of their radius, which covers the
/// soft edge.
const DOT_PAINT_REACH: f64 = 1.8;

/// Pixels added around a shape's projected bounding box so that its soft edge
/// is not clipped.
const PAINT_MARGIN_PX: f64 = 3.0;

/// Depths in meters that a persistent outlier pixel reads, drawn uniformly.
const DEPTH_OUTLIER_RANGE_M: Range<f32> = 0.3..5.0;

/// Inclusive pixel rectangle.
#[derive(Debug, Clone, Copy)]
struct PixelBounds {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

/// Ideal depth in meters per depth pixel, with the pixels that persistently
/// read a wrong value.
pub(super) struct DepthBase {
    /// Zero where the dropout region hides the depth or no surface is hit.
    pub(super) depth_m: Vec<f32>,
    pub(super) persistent_outlier: Vec<bool>,
}

/// Share of a pixel's footprint (`footprint` wide, centered at `center`) that
/// lies within `lo..hi` along one axis.
fn overlap_share(center: f64, footprint: f64, lo: f64, hi: f64) -> f64 {
    let (start, end) = (center - footprint / 2.0, center + footprint / 2.0);
    ((end.min(hi) - start.max(lo)) / footprint).clamp(0.0, 1.0)
}

/// A scene seen from the camera: the surfaces in the color camera's frame and
/// the exposure that scales brightness.
pub(super) struct View {
    pub(super) scene: SyntheticScene,
    pub(super) geometry: CameraGeometry,
    pose: Pose,
    screen: Plane,
    wall: Plane,
    brightness: f64,
}

impl View {
    pub(super) fn new(scene: SyntheticScene, settings: &CaptureSettings) -> Self {
        let pose = scene.pose();
        let geometry = scene.geometry;
        let camera_from_screen = pose.screen_from_camera.inverse();
        let screen = Plane {
            point: camera_from_screen.translation,
            normal: camera_from_screen.rotation * Vector3::z(),
        };
        let wall_world_z = scene.screen_center_m.z + WALL_BEHIND_SCREEN_M;
        let camera_from_world = pose.world_from_camera.transpose();
        let wall = Plane {
            point: camera_from_world * Vector3::new(0.0, 0.0, wall_world_z),
            normal: camera_from_world * Vector3::z(),
        };
        let brightness = f64::from(settings.color_exposure_us) / REFERENCE_EXPOSURE_US
            * f64::from(settings.color_gain.max(1));
        Self {
            scene,
            geometry,
            pose,
            screen,
            wall,
            brightness,
        }
    }

    fn scaled(&self, level: f64) -> u8 {
        (level * self.brightness).clamp(0.0, 255.0) as u8
    }

    /// Screen coordinates (meters) of the color pixel's ray, with the
    /// distance along the ray that reaches it.
    fn color_pixel_on_screen(&self, pixel: Point2<f64>) -> Option<(Vector2<f64>, f64)> {
        let ray = pixel_ray(&self.geometry.color, pixel)?;
        let t = self.screen.ray_parameter(&Vector3::zeros(), &ray)?;
        let p = self.pose.screen_from_camera.apply(&(ray * t));
        Some((Vector2::new(p.x, p.y), t * ray.norm()))
    }

    /// The color image without sensor noise.
    pub(super) fn render_color(&self) -> RgbImage {
        let intr = &self.geometry.color;
        let mut img = RgbImage::new(intr.width, intr.height);
        self.paint_room(&mut img);
        let to_camera = self.pose.screen_from_camera.inverse();
        self.blacken_panel(&mut img, &to_camera);
        if self.scene.show_pattern {
            self.paint_pattern(&mut img, &to_camera);
        }
        img
    }

    /// The room is a dim gradient wall.
    fn paint_room(&self, img: &mut RgbImage) {
        let (w, h) = img.dimensions();
        for y in 0..h {
            for x in 0..w {
                let level = WALL_LEVEL_BASE
                    + WALL_LEVEL_ACROSS * (f64::from(x) / f64::from(w))
                    + WALL_LEVEL_DOWN * (f64::from(y) / f64::from(h));
                img.put_pixel(x, y, Rgb([self.scaled(level); 3]));
            }
        }
    }

    /// The panel's outline in the color image, traced along its distorted
    /// edges.
    fn panel_outline(&self, to_camera: &Extrinsics) -> Vec<Point2<f64>> {
        let [sw, sh] = self.scene.screen_size_m;
        let corners = [(0.0, 0.0), (sw, 0.0), (sw, sh), (0.0, sh)];
        let mut outline: Vec<Point2<f64>> = Vec::new();
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            for step in 0..OUTLINE_STEPS_PER_EDGE {
                let f = f64::from(step) / f64::from(OUTLINE_STEPS_PER_EDGE);
                let p = to_camera.apply(&Vector3::new(
                    a.0 + (b.0 - a.0) * f,
                    a.1 + (b.1 - a.1) * f,
                    0.0,
                ));
                if let Some(px) = project(&self.geometry.color, &p) {
                    outline.push(px);
                }
            }
        }
        outline
    }

    /// Fills the panel's outline with the dark panel level.
    fn blacken_panel(&self, img: &mut RgbImage, to_camera: &Extrinsics) {
        let outline = self.panel_outline(to_camera);
        let (w, h) = img.dimensions();
        let black = Rgb([self.scaled(PANEL_BLACK_LEVEL); 3]);
        for y in 0..h {
            let mut crossings: Vec<f64> = Vec::new();
            for i in 0..outline.len() {
                let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
                let yf = f64::from(y);
                if (a.y <= yf) != (b.y <= yf) {
                    crossings.push(a.x + (yf - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
            crossings.sort_by(f64::total_cmp);
            for pair in crossings.as_chunks::<2>().0 {
                let (x0, x1) = (
                    pair[0].ceil().max(0.0) as u32,
                    (pair[1].floor().min(f64::from(w) - 1.0)).max(-1.0),
                );
                if x1 < 0.0 {
                    continue;
                }
                for x in x0..=(x1 as u32) {
                    img.put_pixel(x, y, black);
                }
            }
        }
    }

    /// Pixel rectangle covering the camera's view of `points` on the panel
    /// (meters in the screen frame), grown by `pad` pixels and clamped to the
    /// image. `None` when a point is behind the camera.
    fn pixel_bounds(
        &self,
        to_camera: &Extrinsics,
        points: &[Vector2<f64>],
        pad: f64,
    ) -> Option<PixelBounds> {
        let intr = &self.geometry.color;
        let mut lo = Vector2::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Vector2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in points {
            let px = project(intr, &to_camera.apply(&Vector3::new(p.x, p.y, 0.0)))?;
            lo = lo.inf(&px.coords);
            hi = hi.sup(&px.coords);
        }
        let clamp = |v: f64, max: u32| v.clamp(0.0, f64::from(max) - 1.0) as u32;
        Some(PixelBounds {
            x0: clamp((lo.x - pad).floor(), intr.width),
            y0: clamp((lo.y - pad).floor(), intr.height),
            x1: clamp((hi.x + pad).ceil(), intr.width),
            y1: clamp((hi.y + pad).ceil(), intr.height),
        })
    }

    /// Draws the pattern's dots and frame rings as the bright parts of the
    /// image.
    fn paint_pattern(&self, img: &mut RgbImage, to_camera: &Extrinsics) {
        // The displayed image is the panel scaled about its center by the
        // overscan. Only what lands on the panel reaches the camera.
        let [image_w, image_h] = self.scene.image_extent_m();
        let origin = self.scene.image_origin_m();
        let pattern = self.scene.pattern();

        for dot in pattern.dots() {
            let center_m = origin + Vector2::new(dot.u * image_w, dot.v * image_h);
            let radius = dot.radius_h * image_h;
            let reach = radius * DOT_PAINT_REACH;
            let corners = [-1.0, 1.0]
                .map(|dx| [-1.0, 1.0].map(|dy| center_m + Vector2::new(dx * reach, dy * reach)));
            let points: Vec<_> = corners.into_iter().flatten().collect();
            let Some(bounds) = self.pixel_bounds(to_camera, &points, PAINT_MARGIN_PX) else {
                continue;
            };
            self.paint(img, bounds, |on_screen, footprint| {
                ((radius - (on_screen - center_m).norm()) / footprint + 0.5).clamp(0.0, 1.0)
            });
        }
        for rect in pattern.frame_rects() {
            let (x0, y0) = (origin.x + rect.x * image_h, origin.y + rect.y * image_h);
            let (x1, y1) = (x0 + rect.width * image_h, y0 + rect.height * image_h);
            let points = [
                Vector2::new(x0, y0),
                Vector2::new(x1, y0),
                Vector2::new(x1, y1),
                Vector2::new(x0, y1),
            ];
            let Some(bounds) = self.pixel_bounds(to_camera, &points, PAINT_MARGIN_PX) else {
                continue;
            };
            self.paint(img, bounds, |on_screen, footprint| {
                overlap_share(on_screen.x, footprint, x0, x1)
                    * overlap_share(on_screen.y, footprint, y0, y1)
            });
        }
    }

    /// Brightens the pixels of `bounds` by the coverage of a shape, given
    /// on-screen position in meters and the pixel's footprint there. Light
    /// only reaches the camera from the panel, so coverage is clipped to it.
    fn paint(
        &self,
        img: &mut RgbImage,
        bounds: PixelBounds,
        coverage: impl Fn(Vector2<f64>, f64) -> f64,
    ) {
        let [sw, sh] = self.scene.screen_size_m;
        for y in bounds.y0..=bounds.y1 {
            for x in bounds.x0..=bounds.x1 {
                let Some((on_screen, range)) =
                    self.color_pixel_on_screen(Point2::new(f64::from(x), f64::from(y)))
                else {
                    continue;
                };
                let footprint = range / self.geometry.color.fx;
                let on_panel = overlap_share(on_screen.x, footprint, 0.0, sw)
                    * overlap_share(on_screen.y, footprint, 0.0, sh);
                let share = coverage(on_screen, footprint) * on_panel;
                if share > 0.0 {
                    let level = self.scaled(PANEL_BLACK_LEVEL + PANEL_LIT_LEVEL_RANGE * share);
                    let current = img.get_pixel(x, y)[0];
                    img.put_pixel(x, y, Rgb([level.max(current); 3]));
                }
            }
        }
    }

    /// Whether the unit-coordinate position `(u, v)` on the screen lies in
    /// the region where the depth sensor returns nothing.
    fn in_depth_dropout(&self, u: f64, v: f64) -> bool {
        self.scene
            .depth_dropout_region
            .is_some_and(|r| u >= r[0] && u <= r[2] && v >= r[1] && v <= r[3])
    }

    /// Distance along `direction` from `origin` (in the color camera's frame)
    /// to the first surface, or zero for none or a dropout.
    fn ideal_depth(&self, origin: &Vector3<f64>, direction: &Vector3<f64>) -> f64 {
        let [sw, sh] = self.scene.screen_size_m;
        if let Some(t) = self.screen.ray_parameter(origin, direction) {
            let p = self
                .pose
                .screen_from_camera
                .apply(&(origin + direction * t));
            if (0.0..=sw).contains(&p.x) && (0.0..=sh).contains(&p.y) {
                return if self.in_depth_dropout(p.x / sw, p.y / sh) {
                    0.0
                } else {
                    t
                };
            }
        }
        self.wall.ray_parameter(origin, direction).unwrap_or(0.0)
    }

    /// Ideal depth per depth pixel, and which pixels read a persistent wrong
    /// value. Draws from `rng` once per pixel that sees a surface.
    pub(super) fn render_depth_base(&self, rng: &mut StdRng) -> DepthBase {
        let intr = self.geometry.depth;
        let (w, h) = (intr.width, intr.height);
        let color_from_depth = self.geometry.color_to_depth.inverse();
        let mut base = DepthBase {
            depth_m: vec![0.0; (w * h) as usize],
            persistent_outlier: vec![false; (w * h) as usize],
        };
        for y in 0..h {
            for x in 0..w {
                let Some(ray) = pixel_ray(&intr, Point2::new(f64::from(x), f64::from(y))) else {
                    continue;
                };
                let origin = color_from_depth.translation;
                let direction = color_from_depth.rotation * ray;
                let i = (y * w + x) as usize;
                base.depth_m[i] = self.ideal_depth(&origin, &direction) as f32;
                if base.depth_m[i] > 0.0 && rng.random::<f64>() < self.scene.depth_outlier_fraction
                {
                    base.depth_m[i] = rng.random_range(DEPTH_OUTLIER_RANGE_M);
                    base.persistent_outlier[i] = true;
                }
            }
        }
        base
    }

    /// Pixels of the six markers followed by the clutter spots in the
    /// infrared image. Markers behind the camera are missing.
    pub(super) fn project_markers(&self) -> Vec<Point2<f64>> {
        let [sw, sh] = self.scene.screen_size_m;
        let to_camera = self.pose.screen_from_camera.inverse();
        let depth_from_color = self.geometry.color_to_depth;
        let ir_from_depth = self.geometry.ir_to_depth.inverse();
        let mut out = Vec::new();
        for [u, v] in SIMULATED_MARKERS {
            // The standoff points toward the viewer, which is -z in the screen frame.
            let p_screen = Vector3::new(u * sw, v * sh, -self.scene.marker_standoff_m);
            let p_ir = ir_from_depth.apply(&depth_from_color.apply(&to_camera.apply(&p_screen)));
            if let Some(px) = project(&self.geometry.ir, &p_ir) {
                out.push(px);
            }
        }
        out.extend(self.scene.ir_clutter.iter().copied());
        out
    }
}
