//! A camera that needs no hardware: it renders what a depth camera with a
//! color imager, an infrared imager and an IMU would see while looking at a
//! screen that shows the dot pattern.
//!
//! The tests drive [`crate::session::Session`] with it, and a GUI can offer it
//! as a demo camera. The scene is a physical screen of known size at a known
//! pose, a camera with a known tilt, and the six IR markers at their layout
//! positions, so [`SyntheticScene::truth`] gives the calibration the solver
//! should recover.
//!
//! Conventions: the *world* frame is gravity aligned with `x` right, `y` down
//! and `z` forward; the camera starts at its origin. Screen and marker
//! coordinates in [`Truth`] use the physical screen frame: origin at the top
//! left corner, `x` right, `y` down, `z` away from the viewer.

use std::time::Duration;

use image::{GrayImage, Luma, Rgb, RgbImage};
use nalgebra::{Matrix3, Point2, Point3, Rotation3, UnitQuaternion, Vector2, Vector3};
use odyssey_common::ScreenCalibration;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::camera::{
    CameraBackend, CameraError, CameraGeometry, CameraInfo, CameraStream, CaptureSettings, DepthImage, Extrinsics,
    Frames, ImuSample, Intrinsics,
};
use crate::markers::MARKER_LAYOUT;
use crate::pattern::Pattern;
use crate::plane::{pixel_ray, project};

const GRAVITY: f64 = 9.81;
const IMU_RATE_HZ: f64 = 200.0;
const FRAME_RATE_HZ: f64 = 30.0;
const DEPTH_UNIT_M: f32 = 0.001;

/// Everything that defines the synthetic scene. Fields are public: build one
/// from [`Default`] and change what a scenario needs.
#[derive(Debug, Clone)]
pub struct SyntheticScene {
    /// Physical width and height of the screen in meters.
    pub screen_size_m: [f64; 2],
    /// Center of the screen in the world frame.
    pub screen_center_m: Vector3<f64>,
    /// Rotation of the screen about the vertical axis, degrees.
    pub screen_yaw_deg: f64,
    /// Backward lean of the screen's top edge, degrees.
    pub screen_pitch_deg: f64,
    /// Camera rotation about the vertical axis, degrees.
    pub camera_yaw_deg: f64,
    /// Camera pitch in degrees; positive looks down.
    pub camera_pitch_deg: f64,
    /// Camera rotation about its optical axis in degrees; positive lowers the
    /// camera's right side.
    pub camera_roll_deg: f64,
    pub geometry: CameraGeometry,
    /// Distance in front of the screen plane at which the markers sit.
    pub marker_standoff_m: f64,
    /// Standard deviation of per-pixel, per-frame depth noise, meters.
    pub depth_noise_m: f64,
    /// Fraction of depth pixels that drop out independently in every frame.
    pub depth_hole_fraction: f64,
    /// Fraction of depth pixels that read a persistent wrong value.
    pub depth_outlier_fraction: f64,
    /// Rectangle `[u0, v0, u1, v1]` of the screen, in unit coordinates, where
    /// the depth sensor returns nothing.
    pub depth_dropout_region: Option<[f64; 4]>,
    /// Standard deviation of color noise in 8-bit levels.
    pub color_noise: f64,
    /// Extra bright spots in the infrared image, in infrared pixels.
    pub ir_clutter: Vec<Point2<f64>>,
    /// Whether the camera reports IMU samples.
    pub imu_enabled: bool,
    /// Whether the screen shows the dot pattern. Without it the color image
    /// shows a plain black screen.
    pub show_pattern: bool,
    /// Sleep one frame interval per frame, as a real camera would.
    pub realtime: bool,
    pub seed: u64,
}

impl Default for SyntheticScene {
    /// A 55-inch 16:9 screen two meters away with a slight yaw, seen by an
    /// upright camera.
    fn default() -> Self {
        let diagonal = 55.0 * 0.0254;
        let aspect: f64 = 16.0 / 9.0;
        let height = diagonal / (aspect * aspect + 1.0).sqrt();
        let color = Intrinsics {
            width: 1280,
            height: 720,
            fx: 920.0,
            fy: 922.0,
            cx: 640.3,
            cy: 359.1,
            k: [-0.05, 0.02, -0.003, 0.0, 0.0, 0.0],
            p: [0.0006, -0.0004],
        };
        let depth = Intrinsics {
            width: 640,
            height: 400,
            fx: 455.0,
            fy: 455.0,
            cx: 320.4,
            cy: 199.2,
            k: [-0.012, 0.004, 0.0, 0.0, 0.0, 0.0],
            p: [0.0, 0.0],
        };
        let tilt = Rotation3::from_euler_angles(0.0035, -0.005, 0.0015);
        Self {
            screen_size_m: [aspect * height, height],
            screen_center_m: Vector3::new(0.0, 0.0, 2.0),
            screen_yaw_deg: 12.0,
            screen_pitch_deg: 0.0,
            camera_yaw_deg: 0.0,
            camera_pitch_deg: 0.0,
            camera_roll_deg: 0.0,
            geometry: CameraGeometry {
                color,
                depth,
                ir: depth,
                color_to_depth: Extrinsics { rotation: tilt.into_inner(), translation: Vector3::new(-0.032, 0.001, -0.002) },
                ir_to_depth: Extrinsics::identity(),
            },
            marker_standoff_m: 0.0,
            depth_noise_m: 0.002,
            depth_hole_fraction: 0.0,
            depth_outlier_fraction: 0.0,
            depth_dropout_region: None,
            color_noise: 1.5,
            ir_clutter: Vec::new(),
            imu_enabled: true,
            show_pattern: true,
            realtime: false,
            seed: 1,
        }
    }
}

/// The calibration the solver should recover from a [`SyntheticScene`].
#[derive(Debug, Clone, Copy)]
pub struct Truth {
    /// Takes color-camera coordinates to the physical screen frame.
    pub screen_from_camera: Extrinsics,
    pub size_m: [f64; 2],
    /// Markers in the physical screen frame, in layout order.
    pub markers_m: [Point3<f64>; 6],
}

impl Truth {
    /// Screen corners (top left, top right, bottom right, bottom left) in
    /// the physical screen frame.
    pub fn corners_m(&self) -> [Point2<f64>; 4] {
        let [w, h] = self.size_m;
        [Point2::new(0.0, 0.0), Point2::new(w, 0.0), Point2::new(w, h), Point2::new(0.0, h)]
    }

    pub fn diagonal_in(&self) -> f64 {
        self.size_m[0].hypot(self.size_m[1]) / 0.0254
    }

    /// The calibration a perfect solver returns when the camera-to-aligned
    /// rotation is `camera_to_aligned`.
    pub fn calibration(&self, camera_to_aligned: &Matrix3<f64>) -> ScreenCalibration<f32> {
        let screen_from_aligned = self.screen_from_camera.rotation * camera_to_aligned.transpose();
        let rotation = UnitQuaternion::from_rotation_matrix(&Rotation3::from_matrix_unchecked(screen_from_aligned));
        let [w, h] = self.size_m;
        ScreenCalibration {
            rotation: rotation.cast(),
            homography: Matrix3::new(1.0 / w, 0.0, 0.0, 0.0, 1.0 / h, 0.0, 0.0, 0.0, 1.0).cast(),
            object_points: self.markers_m.map(|p| p.cast()),
        }
    }
}

struct Pose {
    /// Camera frame to world frame.
    world_from_camera: Matrix3<f64>,
    screen_from_camera: Extrinsics,
}

impl SyntheticScene {
    pub fn aspect(&self) -> f64 {
        self.screen_size_m[0] / self.screen_size_m[1]
    }

    /// The pattern the screen shows.
    pub fn pattern(&self) -> Pattern {
        Pattern::new(self.aspect())
    }

    fn pose(&self) -> Pose {
        let rot = |axis: Vector3<f64>, deg: f64| Rotation3::from_axis_angle(&nalgebra::Unit::new_normalize(axis), deg.to_radians());
        let world_from_camera = (rot(Vector3::y(), self.camera_yaw_deg)
            * rot(Vector3::x(), -self.camera_pitch_deg)
            * rot(Vector3::z(), self.camera_roll_deg))
        .into_inner();
        let world_from_screen = (rot(Vector3::y(), self.screen_yaw_deg) * rot(Vector3::x(), self.screen_pitch_deg)).into_inner();
        let [w, h] = self.screen_size_m;
        let top_left_world = self.screen_center_m - world_from_screen * Vector3::new(w / 2.0, h / 2.0, 0.0);
        let screen_from_world = world_from_screen.transpose();
        Pose {
            world_from_camera,
            screen_from_camera: Extrinsics {
                rotation: screen_from_world * world_from_camera,
                translation: -(screen_from_world * top_left_world),
            },
        }
    }

    pub fn truth(&self) -> Truth {
        let [w, h] = self.screen_size_m;
        Truth {
            screen_from_camera: self.pose().screen_from_camera,
            size_m: self.screen_size_m,
            markers_m: MARKER_LAYOUT.map(|[u, v]| Point3::new(u * w, v * h, 0.0)),
        }
    }

    /// Gravity direction in the camera frame.
    pub fn down_in_camera(&self) -> Vector3<f64> {
        self.pose().world_from_camera.transpose() * Vector3::y()
    }

    /// The camera-to-aligned rotation an ideal gravity estimate yields: the
    /// smallest rotation that points gravity along `+y`.
    pub fn camera_to_aligned(&self) -> Matrix3<f64> {
        UnitQuaternion::rotation_between(&self.down_in_camera(), &Vector3::y())
            .unwrap_or_default()
            .to_rotation_matrix()
            .into_inner()
    }
}

/// Lists one synthetic camera and opens it.
#[derive(Debug, Clone, Default)]
pub struct SyntheticBackend {
    pub scene: SyntheticScene,
}

impl SyntheticBackend {
    pub fn new(scene: SyntheticScene) -> Self {
        Self { scene }
    }

    pub fn truth(&self) -> Truth {
        self.scene.truth()
    }
}

impl CameraBackend for SyntheticBackend {
    fn list(&self) -> Result<Vec<CameraInfo>, CameraError> {
        Ok(vec![CameraInfo { id: "synthetic".into(), name: "Synthetic depth camera".into(), serial: "SYNTHETIC-0".into() }])
    }

    fn open(&self, _camera: &CameraInfo, settings: &CaptureSettings) -> Result<Box<dyn CameraStream>, CameraError> {
        Ok(Box::new(SyntheticStream::new(self.scene.clone(), settings)))
    }
}

/// Plane of the screen (or the wall behind it) in the color camera's frame.
struct ScenePlane {
    normal: Vector3<f64>,
    point: Vector3<f64>,
}

impl ScenePlane {
    /// Ray parameter where `origin + t * direction` meets the plane.
    fn hit(&self, origin: &Vector3<f64>, direction: &Vector3<f64>) -> Option<f64> {
        let denom = self.normal.dot(direction);
        if denom.abs() < 1e-9 {
            return None;
        }
        let t = self.normal.dot(&(self.point - origin)) / denom;
        (t > 0.0).then_some(t)
    }
}

/// A running synthetic camera.
pub struct SyntheticStream {
    scene: SyntheticScene,
    geometry: CameraGeometry,
    pose: Pose,
    screen: ScenePlane,
    wall: ScenePlane,
    brightness: f64,
    color_base: RgbImage,
    depth_base: Vec<f32>,
    depth_persistent_outlier: Vec<bool>,
    marker_px: Vec<Point2<f64>>,
    rng: StdRng,
    clock: Duration,
    next_imu: Duration,
    imu: Vec<ImuSample>,
    ir_exposure_low: bool,
    projector_on: bool,
}

fn gaussian(rng: &mut StdRng) -> f64 {
    let u1: f64 = rng.random_range(f64::EPSILON..1.0);
    let u2: f64 = rng.random();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

impl SyntheticStream {
    pub fn new(scene: SyntheticScene, settings: &CaptureSettings) -> Self {
        let pose = scene.pose();
        let geometry = scene.geometry;
        let r_cs = pose.screen_from_camera.inverse();
        let screen = ScenePlane { normal: r_cs.rotation * Vector3::z(), point: r_cs.translation };
        let wall_world_z = scene.screen_center_m.z + 1.5;
        let r_cw = pose.world_from_camera.transpose();
        let wall = ScenePlane { normal: r_cw * Vector3::z(), point: r_cw * Vector3::new(0.0, 0.0, wall_world_z) };
        let brightness = f64::from(settings.color_exposure_us) / 1500.0 * f64::from(settings.color_gain.max(1));

        let mut stream = Self {
            rng: StdRng::seed_from_u64(scene.seed),
            scene,
            geometry,
            pose,
            screen,
            wall,
            brightness,
            color_base: RgbImage::new(0, 0),
            depth_base: Vec::new(),
            depth_persistent_outlier: Vec::new(),
            marker_px: Vec::new(),
            clock: Duration::ZERO,
            next_imu: Duration::ZERO,
            imu: Vec::new(),
            ir_exposure_low: false,
            projector_on: true,
        };
        stream.color_base = stream.render_color();
        stream.render_depth_base();
        stream.marker_px = stream.project_markers();
        // A camera that has been streaming for a moment has IMU history.
        stream.advance_clock(Duration::from_millis(600));
        stream
    }

    pub fn scene(&self) -> &SyntheticScene {
        &self.scene
    }

    fn scaled(&self, level: f64) -> u8 {
        (level * self.brightness).clamp(0.0, 255.0) as u8
    }

    /// Screen coordinates (meters) of the color pixel's ray, with the ray
    /// parameter that reaches it.
    fn color_pixel_on_screen(&self, pixel: Point2<f64>) -> Option<(Vector2<f64>, f64, f64)> {
        let ray = pixel_ray(&self.geometry.color, pixel)?;
        let t = self.screen.hit(&Vector3::zeros(), &ray)?;
        let p = self.pose.screen_from_camera.apply(&(ray * t));
        Some((Vector2::new(p.x, p.y), t * ray.norm(), p.z))
    }

    fn render_color(&self) -> RgbImage {
        let intr = &self.geometry.color;
        let (w, h) = (intr.width, intr.height);
        let [sw, sh] = self.scene.screen_size_m;
        let mut img = RgbImage::new(w, h);

        // Room: a dim gradient wall.
        for y in 0..h {
            for x in 0..w {
                let level = 60.0 + 40.0 * (x as f64 / w as f64) + 15.0 * (y as f64 / h as f64);
                img.put_pixel(x, y, Rgb([self.scaled(level); 3]));
            }
        }

        // Screen polygon, traced along its distorted edges.
        let corners = [(0.0, 0.0), (sw, 0.0), (sw, sh), (0.0, sh)];
        let to_camera = self.pose.screen_from_camera.inverse();
        let mut outline: Vec<Point2<f64>> = Vec::new();
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            for s in 0..48 {
                let f = s as f64 / 48.0;
                let p = to_camera.apply(&Vector3::new(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, 0.0));
                if let Some(px) = project(intr, &p) {
                    outline.push(px);
                }
            }
        }
        let black = Rgb([self.scaled(5.0); 3]);
        for y in 0..h {
            let mut xs: Vec<f64> = Vec::new();
            for i in 0..outline.len() {
                let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
                let yf = y as f64;
                if (a.y <= yf) != (b.y <= yf) {
                    xs.push(a.x + (yf - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
            xs.sort_by(f64::total_cmp);
            for pair in xs.as_chunks::<2>().0 {
                let (x0, x1) = (pair[0].ceil().max(0.0) as u32, (pair[1].floor().min(w as f64 - 1.0)).max(-1.0));
                if x1 < 0.0 {
                    continue;
                }
                for x in x0..=(x1 as u32) {
                    img.put_pixel(x, y, black);
                }
            }
        }

        if !self.scene.show_pattern {
            return img;
        }
        for dot in self.scene.pattern().dots() {
            let center_m = Vector2::new(dot.u * sw, dot.v * sh);
            let radius = dot.radius_h * sh;
            let center_cam = to_camera.apply(&Vector3::new(center_m.x, center_m.y, 0.0));
            let Some(center_px) = project(intr, &center_cam) else { continue };
            let half = (radius * 1.8 * intr.fx / center_cam.z).ceil() + 3.0;
            let (x0, x1) = ((center_px.x - half).floor().max(0.0) as u32, (center_px.x + half).ceil().min(w as f64 - 1.0) as u32);
            let (y0, y1) = ((center_px.y - half).floor().max(0.0) as u32, (center_px.y + half).ceil().min(h as f64 - 1.0) as u32);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let Some((on_screen, range, _)) = self.color_pixel_on_screen(Point2::new(x as f64, y as f64)) else { continue };
                    let footprint = range / intr.fx;
                    let coverage = ((radius - (on_screen - center_m).norm()) / footprint + 0.5).clamp(0.0, 1.0);
                    if coverage > 0.0 {
                        let level = self.scaled(5.0 + 230.0 * coverage);
                        let current = img.get_pixel(x, y)[0];
                        img.put_pixel(x, y, Rgb([level.max(current); 3]));
                    }
                }
            }
        }
        img
    }

    /// Ideal depth in meters per depth pixel; zero where the dropout region
    /// hides it.
    fn render_depth_base(&mut self) {
        let intr = self.geometry.depth;
        let (w, h) = (intr.width, intr.height);
        let color_from_depth = self.geometry.color_to_depth.inverse();
        let [sw, sh] = self.scene.screen_size_m;
        self.depth_base = vec![0.0; (w * h) as usize];
        self.depth_persistent_outlier = vec![false; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let Some(ray) = pixel_ray(&intr, Point2::new(x as f64, y as f64)) else { continue };
                let origin = color_from_depth.translation;
                let direction = color_from_depth.rotation * ray;
                let mut depth = None;
                if let Some(t) = self.screen.hit(&origin, &direction) {
                    let p = self.pose.screen_from_camera.apply(&(origin + direction * t));
                    if (0.0..=sw).contains(&p.x) && (0.0..=sh).contains(&p.y) {
                        let (u, v) = (p.x / sw, p.y / sh);
                        let hidden = self
                            .scene
                            .depth_dropout_region
                            .is_some_and(|r| u >= r[0] && u <= r[2] && v >= r[1] && v <= r[3]);
                        depth = Some(if hidden { 0.0 } else { t });
                    }
                }
                if depth.is_none() {
                    depth = self.wall.hit(&origin, &direction);
                }
                let i = (y * w + x) as usize;
                self.depth_base[i] = depth.unwrap_or(0.0) as f32;
                if self.depth_base[i] > 0.0 && self.rng.random::<f64>() < self.scene.depth_outlier_fraction {
                    self.depth_base[i] = self.rng.random_range(0.3f32..5.0);
                    self.depth_persistent_outlier[i] = true;
                }
            }
        }
    }

    fn project_markers(&self) -> Vec<Point2<f64>> {
        let [sw, sh] = self.scene.screen_size_m;
        let to_camera = self.pose.screen_from_camera.inverse();
        let depth_from_color = self.geometry.color_to_depth;
        let ir_from_depth = self.geometry.ir_to_depth.inverse();
        let mut out = Vec::new();
        for [u, v] in MARKER_LAYOUT {
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

    fn render_ir(&mut self) -> GrayImage {
        let intr = self.geometry.ir;
        let (w, h) = (intr.width, intr.height);
        let (floor, noise) = if self.ir_exposure_low { (3.0, 3.0) } else { (45.0, 12.0) };
        let mut img = GrayImage::new(w, h);
        for p in img.pixels_mut() {
            let mut v = floor + self.rng.random::<f64>() * noise;
            if self.projector_on && !self.ir_exposure_low && self.rng.random::<f64>() < 0.02 {
                v += 60.0;
            }
            *p = Luma([v as u8]);
        }
        let sigma = 1.6f64;
        for c in &self.marker_px {
            let half = 7;
            for dy in -half..=half {
                for dx in -half..=half {
                    let (x, y) = ((c.x.round() as i32 + dx), (c.y.round() as i32 + dy));
                    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                        continue;
                    }
                    let d2 = (x as f64 - c.x).powi(2) + (y as f64 - c.y).powi(2);
                    let add = 255.0 * 1.8 * (-d2 / (2.0 * sigma * sigma)).exp();
                    let cur = img.get_pixel(x as u32, y as u32)[0] as f64;
                    img.put_pixel(x as u32, y as u32, Luma([(cur + add).min(255.0) as u8]));
                }
            }
        }
        img
    }

    fn render_depth(&mut self) -> DepthImage {
        let (w, h) = (self.geometry.depth.width, self.geometry.depth.height);
        let mut img = DepthImage::new(w, h);
        let (sigma, holes) = (self.scene.depth_noise_m, self.scene.depth_hole_fraction);
        for i in 0..(w * h) as usize {
            let base = f64::from(self.depth_base[i]);
            if base <= 0.0 || (holes > 0.0 && self.rng.random::<f64>() < holes) {
                continue;
            }
            let noisy = if self.depth_persistent_outlier[i] { base } else { base + sigma * gaussian(&mut self.rng) };
            let raw = (noisy / f64::from(DEPTH_UNIT_M)).round().clamp(1.0, f64::from(u16::MAX)) as u16;
            img.as_mut()[i] = raw;
        }
        img
    }

    fn render_color_frame(&mut self) -> RgbImage {
        let mut img = self.color_base.clone();
        if self.scene.color_noise > 0.0 {
            let amplitude = self.scene.color_noise * 3.0f64.sqrt();
            for p in img.pixels_mut() {
                let n = self.rng.random_range(-amplitude..=amplitude);
                let v = (f64::from(p[0]) + n).round().clamp(0.0, 255.0) as u8;
                *p = Rgb([v; 3]);
            }
        }
        img
    }

    fn advance_clock(&mut self, by: Duration) {
        self.clock += by;
        if !self.scene.imu_enabled {
            return;
        }
        let down = self.scene.down_in_camera();
        let period = Duration::from_secs_f64(1.0 / IMU_RATE_HZ);
        while self.next_imu <= self.clock {
            let noise = |rng: &mut StdRng, s: f64| s * gaussian(rng);
            let accel = -down * GRAVITY
                + Vector3::new(noise(&mut self.rng, 0.03), noise(&mut self.rng, 0.03), noise(&mut self.rng, 0.03));
            let gyro = Vector3::new(0.001, -0.0005, 0.0007)
                + Vector3::new(noise(&mut self.rng, 0.002), noise(&mut self.rng, 0.002), noise(&mut self.rng, 0.002));
            self.imu.push(ImuSample { timestamp: self.next_imu, gyro, accel });
            self.next_imu += period;
        }
    }
}

impl CameraStream for SyntheticStream {
    fn geometry(&self) -> &CameraGeometry {
        &self.geometry
    }

    fn next_frames(&mut self, _timeout: Duration) -> Result<Frames, CameraError> {
        let interval = Duration::from_secs_f64(1.0 / FRAME_RATE_HZ);
        if self.scene.realtime {
            std::thread::sleep(interval);
        }
        self.advance_clock(interval);
        let color = self.render_color_frame();
        let depth = self.render_depth();
        let ir = self.render_ir();
        Ok(Frames { color, depth, depth_unit_m: DEPTH_UNIT_M, ir })
    }

    fn drain_imu(&mut self) -> Vec<ImuSample> {
        std::mem::take(&mut self.imu)
    }

    fn set_ir_exposure_us(&mut self, exposure_us: u32) -> Result<(), CameraError> {
        self.ir_exposure_low = exposure_us <= 1000;
        Ok(())
    }

    fn set_ir_auto_exposure(&mut self) -> Result<(), CameraError> {
        self.ir_exposure_low = false;
        Ok(())
    }

    fn set_projector_enabled(&mut self, enabled: bool) -> Result<bool, CameraError> {
        self.projector_on = enabled;
        Ok(true)
    }
}
