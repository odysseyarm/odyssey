use std::f64::consts::PI;

use image::{GrayImage, Rgb, RgbImage};
use nalgebra::{Rotation3, UnitQuaternion};
use rand::rngs::StdRng;
use rand::{Rng as _, SeedableRng as _};

use super::*;
use crate::camera::{CameraGeometry, DepthImage, Extrinsics, Frames};
use crate::plane::{Plane, apply_homography, pixel_ray, project, undistort_pixel};

const PAPER: f32 = 225.0;
const INK: f32 = 20.0;
const SURROUND: f32 = 45.0;

fn intrinsics_720() -> Intrinsics {
    Intrinsics {
        width: 1280,
        height: 720,
        fx: 920.0,
        fy: 921.0,
        cx: 640.3,
        cy: 359.1,
        k: [-0.05, 0.02, -0.003, 0.0, 0.0, 0.0],
        p: [0.0006, -0.0004],
    }
}

fn intrinsics_1080() -> Intrinsics {
    Intrinsics {
        width: 1920,
        height: 1080,
        fx: 1380.0,
        fy: 1381.0,
        cx: 960.4,
        cy: 539.2,
        k: [-0.04, 0.015, -0.002, 0.0, 0.0, 0.0],
        p: [0.0004, -0.0003],
    }
}

/// Board-to-camera rotation: spin in the board's own plane by `roll`, then
/// yaw about the camera's vertical axis and pitch about its horizontal
/// axis, all degrees.
fn pose_rotation(yaw: f64, pitch: f64, roll: f64) -> Matrix3<f64> {
    let about = |axis: Vector3<f64>, deg: f64| {
        UnitQuaternion::from_axis_angle(&nalgebra::Unit::new_normalize(axis), deg.to_radians())
            .to_rotation_matrix()
            .into_inner()
    };
    about(Vector3::y(), yaw) * about(Vector3::x(), pitch) * about(Vector3::z(), roll)
}

#[derive(Clone)]
struct Scene {
    intrinsics: Intrinsics,
    rotation: Matrix3<f64>,
    center_m: Vector3<f64>,
    /// Physical dot pitch of the print being rendered.
    pitch_mm: f64,
    /// Fractional brightness change across the image.
    gradient: f64,
    noise: f64,
    seed: u64,
    /// Gray sheets laid on the board: rectangles `[x0, y0, x1, y1]` in
    /// millimeters from the grid center.
    occluders: Vec<[f64; 4]>,
}

impl Scene {
    fn new(intrinsics: Intrinsics, yaw: f64, pitch: f64, roll: f64, distance_m: f64) -> Self {
        Self {
            intrinsics,
            rotation: pose_rotation(yaw, pitch, roll),
            center_m: Vector3::new(0.05, -0.03, distance_m),
            pitch_mm: BoardSpec::nominal_pitch_mm(),
            gradient: 0.35,
            noise: 2.0,
            seed: 7,
            occluders: Vec::new(),
        }
    }

    fn true_distance(&self) -> f64 {
        self.center_m.norm()
    }

    fn board_to_camera_mm(&self, x: f64, y: f64) -> Vector3<f64> {
        self.rotation * Vector3::new(x, y, 0.0) + self.center_m * 1000.0
    }

    fn grid_scale(&self) -> f64 {
        self.pitch_mm / BoardSpec::nominal_pitch_mm()
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Disk { x: f64, y: f64, r: f64 },
    Rect { x0: f64, y0: f64, x1: f64, y1: f64 },
}

impl Shape {
    fn contains(&self, u: f64, v: f64) -> bool {
        match *self {
            Self::Disk { x, y, r } => (u - x).powi(2) + (v - y).powi(2) <= r * r,
            Self::Rect { x0, y0, x1, y1 } => u >= x0 && u <= x1 && v >= y0 && v <= y1,
        }
    }

    fn bounds(&self) -> [f64; 4] {
        match *self {
            Self::Disk { x, y, r } => [x - r, y - r, x + r, y + r],
            Self::Rect { x0, y0, x1, y1 } => [x0, y0, x1, y1],
        }
    }
}

/// Everything printed in black, in millimeters from the grid center: the
/// dots, the scale bar and a few text-like marks near the bottom edge.
fn printed_shapes(scene: &Scene) -> Vec<Shape> {
    let spec = BoardSpec::get();
    let s = scene.grid_scale();
    let mut shapes = Vec::new();
    for row in 0..spec.rows {
        for col in 0..spec.cols {
            if spec.is_missing(col, row) {
                continue;
            }
            let p = spec.dot_position(col, row) * scene.pitch_mm;
            shapes.push(Shape::Disk {
                x: p.x,
                y: p.y,
                r: spec.dot_diameter_mm * s / 2.0,
            });
        }
    }
    shapes.push(Shape::Rect {
        x0: -100.0 * s,
        y0: 95.0 * s,
        x1: 100.0 * s,
        y1: 98.0 * s,
    });
    for i in 0..5 {
        let x = (-90.0 + 9.0 * f64::from(i)) * s;
        shapes.push(Shape::Rect {
            x0: x,
            y0: 100.0 * s,
            x1: x + 3.0 * s,
            y1: 106.0 * s,
        });
    }
    shapes.push(Shape::Disk {
        x: -20.0 * s,
        y: 103.0 * s,
        r: 2.5 * s,
    });
    shapes
}

fn fill_polygon(img: &mut [f32], w: usize, h: usize, polygon: &[Point2<f64>], value: f32) {
    for y in 0..h {
        let yf = y as f64;
        let mut crossings = Vec::new();
        for i in 0..polygon.len() {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            if (a.y <= yf) != (b.y <= yf) {
                crossings.push(a.x + (yf - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
        crossings.sort_by(f64::total_cmp);
        for pair in crossings.as_chunks::<2>().0 {
            let from = pair[0].ceil().max(0.0) as usize;
            let to = pair[1].floor().min(w as f64 - 1.0);
            if to < 0.0 {
                continue;
            }
            for x in from..=(to as usize) {
                img[y * w + x] = value;
            }
        }
    }
}

/// Renders the printed board as a color camera would see it: lens
/// distortion, antialiased ink (8 x 8 supersampling), uneven lighting and
/// noise.
fn render(scene: &Scene) -> RgbImage {
    let k = &scene.intrinsics;
    let (w, h) = (k.width as usize, k.height as usize);
    let mut img = vec![SURROUND; w * h];
    let s = scene.grid_scale();
    let (hx, hy) = (
        BoardSpec::get().board_mm[0] / 2.0 * s,
        BoardSpec::get().board_mm[1] / 2.0 * s,
    );
    let mut outline = Vec::new();
    for (a, b) in [
        ((-hx, -hy), (hx, -hy)),
        ((hx, -hy), (hx, hy)),
        ((hx, hy), (-hx, hy)),
        ((-hx, hy), (-hx, -hy)),
    ] {
        for i in 0..40 {
            let f = f64::from(i) / 40.0;
            let p = scene.board_to_camera_mm(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            outline.push(project(k, &p).expect("the board is in front"));
        }
    }
    fill_polygon(&mut img, w, h, &outline, PAPER);

    let h_mm = pose_homography(k, &scene.rotation, &(scene.center_m * 1000.0));
    let to_board = h_mm.try_inverse().expect("a pose homography inverts");
    for shape in printed_shapes(scene) {
        let b = shape.bounds();
        let mut lo = (f64::INFINITY, f64::INFINITY);
        let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (u, v) in [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])] {
            let p = project(k, &scene.board_to_camera_mm(u, v)).expect("in front");
            lo = (lo.0.min(p.x), lo.1.min(p.y));
            hi = (hi.0.max(p.x), hi.1.max(p.y));
        }
        let (x0, y0) = (
            (lo.0.floor() as i64 - 2).max(1),
            (lo.1.floor() as i64 - 2).max(1),
        );
        let (x1, y1) = (
            (hi.0.ceil() as i64 + 2).min(w as i64 - 2),
            (hi.1.ceil() as i64 + 2).min(h as i64 - 2),
        );
        if x1 < x0 || y1 < y0 {
            continue;
        }
        // Undistorted pixel of every integer position, interpolated for
        // the sub-samples (the lens map is smooth over a pixel).
        let (vw, vh) = ((x1 - x0 + 3) as usize, (y1 - y0 + 3) as usize);
        let mut vertices = Vec::with_capacity(vw * vh);
        for vy in 0..vh {
            for vx in 0..vw {
                let p = Point2::new((x0 - 1 + vx as i64) as f64, (y0 - 1 + vy as i64) as f64);
                vertices.push(undistort_pixel(k, p).expect("inside the image"));
            }
        }
        const N: usize = 8;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let mut inside = 0;
                for j in 0..N {
                    for i in 0..N {
                        let sx = x as f64 - 0.5 + (i as f64 + 0.5) / N as f64;
                        let sy = y as f64 - 0.5 + (j as f64 + 0.5) / N as f64;
                        let (fx0, fy0) = (sx.floor(), sy.floor());
                        let (tx, ty) = (sx - fx0, sy - fy0);
                        let at = |dx: i64, dy: i64| {
                            let ix = (fx0 as i64 + dx - (x0 - 1)) as usize;
                            let iy = (fy0 as i64 + dy - (y0 - 1)) as usize;
                            vertices[iy * vw + ix].coords
                        };
                        let p = at(0, 0) * (1.0 - tx) * (1.0 - ty)
                            + at(1, 0) * tx * (1.0 - ty)
                            + at(0, 1) * (1.0 - tx) * ty
                            + at(1, 1) * tx * ty;
                        let g = apply_homography(&to_board, Point2::from(p)).expect("board point");
                        if shape.contains(g.x, g.y) {
                            inside += 1;
                        }
                    }
                }
                let c = inside as f32 / (N * N) as f32;
                let px = &mut img[y as usize * w + x as usize];
                *px = *px * (1.0 - c) + INK * c;
            }
        }
    }
    for o in &scene.occluders {
        let mut polygon = Vec::new();
        for (a, b) in [
            ((o[0], o[1]), (o[2], o[1])),
            ((o[2], o[1]), (o[2], o[3])),
            ((o[2], o[3]), (o[0], o[3])),
            ((o[0], o[3]), (o[0], o[1])),
        ] {
            for i in 0..40 {
                let f = f64::from(i) / 40.0;
                let p = scene.board_to_camera_mm(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
                polygon.push(project(k, &p).expect("the board is in front"));
            }
        }
        fill_polygon(&mut img, w, h, &polygon, 120.0);
    }

    let mut rng = StdRng::seed_from_u64(scene.seed);
    let mut out = RgbImage::new(k.width, k.height);
    for y in 0..h {
        for x in 0..w {
            let light = 1.0
                + scene.gradient * ((x as f64 / w as f64) - 0.5)
                + 0.5 * scene.gradient * ((y as f64 / h as f64) - 0.5);
            let (u1, u2): (f64, f64) = (rng.random::<f64>().max(1e-12), rng.random());
            let noise = scene.noise * (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos();
            let v = (f64::from(img[y * w + x]) * light + noise)
                .round()
                .clamp(0.0, 255.0) as u8;
            out.put_pixel(x as u32, y as u32, Rgb([v, v, v]));
        }
    }
    out
}

fn test_geometry(color: Intrinsics) -> CameraGeometry {
    let depth = Intrinsics {
        width: 640,
        height: 400,
        fx: 455.0,
        fy: 455.0,
        cx: 320.4,
        cy: 199.2,
        k: [0.0; 6],
        p: [0.0; 2],
    };
    CameraGeometry {
        color,
        depth,
        ir: depth,
        color_to_depth: Extrinsics {
            rotation: Rotation3::from_euler_angles(0.004, -0.003, 0.002).into_inner(),
            translation: Vector3::new(-0.032, 0.002, 0.003),
        },
        ir_to_depth: Extrinsics::identity(),
    }
}

/// Depth image of the board's plane seen from the depth camera, every
/// depth scaled by `1 + bias` about the depth camera (a sensor scale
/// error), with noise, dropouts and a few wrong values on the board, and
/// a far wall around it.
fn render_depth(scene: &Scene, geometry: &CameraGeometry, bias: f64, seed: u64) -> DepthImage {
    let mut rng = StdRng::seed_from_u64(seed);
    let intr = &geometry.depth;
    let mut img = DepthImage::new(intr.width, intr.height);
    let color_from_depth = geometry.color_to_depth.inverse();
    let plane = Plane::new(scene.center_m, -(scene.rotation * Vector3::z()))
        .transformed(&geometry.color_to_depth);
    let s = scene.grid_scale();
    let half = [
        BoardSpec::get().board_mm[0] / 2.0,
        BoardSpec::get().board_mm[1] / 2.0,
    ];
    for y in 0..intr.height {
        for x in 0..intr.width {
            let ray = pixel_ray(intr, Point2::new(f64::from(x), f64::from(y))).expect("pinhole");
            let mut z = 3.0;
            if let Some(p) = plane.intersect_ray(&Vector3::zeros(), &ray) {
                let q = scene.rotation.transpose() * (color_from_depth.apply(&p) - scene.center_m);
                if q.x.abs() * 1000.0 <= half[0] * s && q.y.abs() * 1000.0 <= half[1] * s {
                    z = p.z;
                    let roll: f64 = rng.random();
                    if roll < 0.02 {
                        z *= 0.8 + 0.4 * rng.random::<f64>();
                    } else if roll < 0.06 {
                        continue;
                    }
                }
            }
            let (u1, u2): (f64, f64) = (rng.random::<f64>().max(1e-12), rng.random());
            let noise = 0.0015 * (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos();
            let raw = ((z * (1.0 + bias) + noise) * 1000.0).round();
            img.put_pixel(x, y, image::Luma([raw as u16]));
        }
    }
    img
}

fn frame(scene: &Scene, geometry: &CameraGeometry, bias: f64, seed: u64) -> Frames {
    let mut scene = scene.clone();
    scene.seed = seed;
    Frames {
        color: render(&scene),
        depth: render_depth(&scene, geometry, bias, seed),
        depth_unit_m: 0.001,
        ir: GrayImage::new(1, 1),
    }
}

/// Relative error of the recovered distance and the angle between the
/// recovered and true board rotations, degrees.
fn pose_error(detection: &BoardDetection, scene: &Scene) -> (f64, f64) {
    let pose = detection.pose(scene.pitch_mm).expect("pose");
    let relative = (pose.distance_m - scene.true_distance()) / scene.true_distance();
    let delta = pose.rotation.transpose() * scene.rotation;
    let angle = ((delta.trace() - 1.0) / 2.0)
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    (relative, angle)
}

/// Distance tolerance derived from the dot size: a half-pixel systematic
/// error in the dot centers, spread over the grid's width of six pitches,
/// moves the apparent scale by `0.5 px / (6 pitch_px)`.
fn distance_tolerance(scene: &Scene) -> f64 {
    let pitch_px = scene.intrinsics.fx * scene.pitch_mm / 1000.0 / scene.center_m.z;
    0.5 / (6.0 * pitch_px)
}

/// Hides the dots in `cols` x `rows` (inclusive ranges) behind a gray
/// sheet laid on the board, cut at the middle of the gaps between dots.
/// A range that reaches the grid's edge extends past the board.
fn hide(scene: &mut Scene, cols: (u32, u32), rows: (u32, u32)) {
    let spec = BoardSpec::get();
    let p = scene.pitch_mm;
    let lo = spec.dot_position(cols.0, rows.0);
    let hi = spec.dot_position(cols.1, rows.1);
    let edge = |at_edge: bool, v: f64, sign: f64| {
        if at_edge {
            sign * 10.0 * p
        } else {
            (v + sign * 0.5) * p
        }
    };
    scene.occluders.push([
        edge(cols.0 == 0, lo.x, -1.0),
        edge(rows.0 == 0, lo.y, -1.0),
        edge(cols.1 == spec.cols - 1, hi.x, 1.0),
        edge(rows.1 == spec.rows - 1, hi.y, 1.0),
    ]);
}

fn assert_accurate(detection: &BoardDetection, scene: &Scene, label: &str) -> (f64, f64) {
    let (relative, angle) = pose_error(detection, scene);
    let tolerance = distance_tolerance(scene);
    eprintln!(
        "{label}: {} dots, diameter {:.1} px, rms {:.3} px, distance error {:+.4}% (tolerance {:.3}%), rotation error {:.3} deg",
        detection.dots.len(),
        detection.median_dot_diameter_px,
        detection.rms_px,
        relative * 100.0,
        tolerance * 100.0,
        angle
    );
    assert!(
        relative.abs() < tolerance,
        "{label}: distance error {relative} exceeds {tolerance}"
    );
    assert!(angle < 1.0, "{label}: rotation error {angle} degrees");
    (relative, angle)
}

#[test]
fn spec_matches_the_artwork_contract() {
    let spec = BoardSpec::get();
    assert_eq!((spec.cols, spec.rows), (7, 5));
    assert_eq!(spec.missing, vec![[6, 0]]);
    assert_eq!(spec.dots_expected(), 34);
    assert_eq!(BoardSpec::nominal_pitch_mm(), 40.0);
    assert_eq!(spec.dot_diameter_pitches(), 0.5);
    // The grid is centered, so the center dot is the origin.
    assert_eq!(spec.dot_position(3, 2), Vector2::zeros());
    assert_eq!(spec.dot_position(0, 0), Vector2::new(-3.0, -2.0));
}

#[test]
fn finds_every_dot_and_the_distance_at_720p() {
    let scene = Scene::new(intrinsics_720(), 15.0, 8.0, 0.0, 1.2);
    let detection = detect_board(&render(&scene), &scene.intrinsics).expect("board found");
    assert_eq!(detection.dots.len(), 34);
    assert_eq!(detection.dots_expected, 34);
    assert_accurate(&detection, &scene, "720p");
    // Grid-center distance and the pixel footprint are consistent: the
    // printed board is 7.62 pitches wide, about 234 px per 7.62 pitches at
    // 30.7 px per pitch.
    let pitch_px = 920.0 * 0.04 / 1.2;
    assert!((detection.footprint_extent_px[0] / (7.62 * pitch_px) - 1.0).abs() < 0.1);
    assert_eq!(detection.target_size(), TargetSize::Adequate);
    assert!((detection.median_dot_diameter_px - 15.0).abs() < 1.5);
}

#[test]
fn finds_every_dot_and_the_distance_at_1080p() {
    let scene = Scene::new(intrinsics_1080(), -25.0, 12.0, 0.0, 2.0);
    let detection = detect_board(&render(&scene), &scene.intrinsics).expect("board found");
    assert_eq!(detection.dots.len(), 34);
    assert_accurate(&detection, &scene, "1080p");
}

#[test]
fn survives_a_strong_lighting_gradient_and_heavy_noise() {
    let mut scene = Scene::new(intrinsics_720(), 20.0, -10.0, 0.0, 1.5);
    scene.gradient = 0.8;
    scene.noise = 6.0;
    let detection = detect_board(&render(&scene), &scene.intrinsics).expect("board found");
    assert_eq!(detection.dots.len(), 34);
    assert_accurate(&detection, &scene, "gradient+noise");
}

#[test]
fn resolves_the_missing_dot_for_every_in_plane_rotation() {
    for roll in [0.0, 37.0, 90.0, 180.0, 270.0] {
        let scene = Scene::new(intrinsics_720(), 12.0, 6.0, roll, 1.2);
        let detection = detect_board(&render(&scene), &scene.intrinsics).expect("board found");
        assert_eq!(detection.dots.len(), 34, "roll {roll}");
        // A wrong orientation would show up as a rotation error near
        // 180 degrees (or 90), not a fraction of a degree.
        assert_accurate(&detection, &scene, &format!("roll {roll}"));
        let corner = detection
            .dots
            .iter()
            .find(|d| (d.col, d.row) == (6, 1))
            .expect("dot next to the missing one");
        let truth = project(
            &scene.intrinsics,
            &scene.board_to_camera_mm(3.0 * 40.0, -40.0),
        )
        .unwrap();
        let truth = undistort_pixel(&scene.intrinsics, truth).unwrap();
        assert!((corner.center_px - truth).norm() < 0.5, "roll {roll}");
    }
}

#[test]
fn a_partly_hidden_board_still_works_above_the_minimum() {
    let mut scene = Scene::new(intrinsics_720(), 8.0, 5.0, 0.0, 1.2);
    hide(&mut scene, (0, 2), (0, 1));
    let detection = detect_board(&render(&scene), &scene.intrinsics).expect("board found");
    assert_eq!(detection.dots.len(), 28);
    assert_accurate(&detection, &scene, "28 dots");

    // The hole in the grid and the image border settle where the grid is
    // even when a whole side is cut off by the frame.
    let mut scene = Scene::new(intrinsics_720(), 8.0, 5.0, 0.0, 0.6);
    scene.center_m.x = 0.33;
    let detection = detect_board(&render(&scene), &scene.intrinsics).expect("board found");
    assert!(detection.dots.len() >= MIN_DOTS && detection.dots.len() < 34);
    assert_accurate(&detection, &scene, "cut by the frame");
}

#[test]
fn hiding_the_dot_opposite_the_hole_leaves_the_orientation_open() {
    // With the corner opposite the missing dot covered, the board turned
    // by 180 degrees explains the visible dots equally well; guessing
    // would move the grid center by up to four pitches.
    let mut scene = Scene::new(intrinsics_720(), 8.0, 5.0, 0.0, 1.2);
    hide(&mut scene, (0, 2), (3, 4));
    let error = detect_board(&render(&scene), &scene.intrinsics).expect_err("ambiguous");
    assert!(
        matches!(error, BoardError::AmbiguousPlacement { matched: 28 }),
        "{error}"
    );
}

#[test]
fn too_few_visible_dots_fail_cleanly() {
    let mut scene = Scene::new(intrinsics_720(), 8.0, 5.0, 0.0, 1.2);
    // Two columns of five dots remain.
    hide(&mut scene, (2, 6), (0, 4));
    let error = detect_board(&render(&scene), &scene.intrinsics).expect_err("ten dots");
    assert!(
        matches!(
            error,
            BoardError::TooFewDots {
                found: 10,
                needed: MIN_DOTS
            }
        ),
        "{error}"
    );
}

#[test]
fn a_view_without_the_missing_dot_is_ambiguous_not_guessed() {
    // Three columns of four dots, away from the hole: the same picture is
    // explained by the grid shifted one pitch, which changes the distance.
    let mut scene = Scene::new(intrinsics_720(), 8.0, 5.0, 0.0, 1.2);
    hide(&mut scene, (3, 6), (0, 4));
    hide(&mut scene, (0, 2), (0, 0));
    let error = detect_board(&render(&scene), &scene.intrinsics).expect_err("ambiguous");
    assert!(
        matches!(error, BoardError::AmbiguousPlacement { .. }),
        "{error}"
    );
}

#[test]
fn frames_without_a_board_report_no_board() {
    let intrinsics = intrinsics_720();
    let blank = RgbImage::from_pixel(1280, 720, Rgb([200, 200, 200]));
    assert!(matches!(
        detect_board(&blank, &intrinsics),
        Err(BoardError::NoBoard { blobs: 0 })
    ));

    // Round dark marks scattered without a grid are not a board.
    let mut rng = StdRng::seed_from_u64(3);
    let mut clutter = blank.clone();
    for _ in 0..25 {
        let (cx, cy) = (
            rng.random_range(60.0..1220.0),
            rng.random_range(60.0..660.0),
        );
        for y in -9i32..=9 {
            for x in -9i32..=9 {
                if x * x + y * y <= 64 {
                    clutter.put_pixel(
                        (cx + f64::from(x)) as u32,
                        (cy + f64::from(y)) as u32,
                        Rgb([30, 30, 30]),
                    );
                }
            }
        }
    }
    let error = detect_board(&clutter, &intrinsics).expect_err("no grid in random marks");
    assert!(
        matches!(
            error,
            BoardError::NoBoard { .. } | BoardError::TooFewDots { .. }
        ),
        "{error}"
    );

    // A board wholly covered.
    let mut scene = Scene::new(intrinsics, 8.0, 5.0, 0.0, 1.2);
    hide(&mut scene, (0, 6), (0, 4));
    assert!(matches!(
        detect_board(&render(&scene), &scene.intrinsics),
        Err(BoardError::NoBoard { .. })
    ));
}

#[test]
fn a_board_seen_too_small_is_flagged_not_trusted() {
    // Dot diameters of about 15, 7.5, 6, 5.3, 4.1 and 3 px.
    let mut summary = Vec::new();
    for distance in [1.2, 2.4, 3.0, 3.5, 4.5, 6.0] {
        let scene = Scene::new(intrinsics_720(), 15.0, 8.0, 0.0, distance);
        let result = detect_board(&render(&scene), &scene.intrinsics);
        match result {
            Ok(detection) => {
                let (relative, _) = pose_error(&detection, &scene);
                let size = detection.target_size();
                summary.push(format!(
                    "{distance} m: found {} dots, diameter {:.1} px, {size:?}, distance error {:+.3}%",
                    detection.dots.len(),
                    detection.median_dot_diameter_px,
                    relative * 100.0
                ));
                if detection.median_dot_diameter_px >= MIN_DOT_DIAMETER_PX {
                    assert!(
                        relative.abs() < 3.0 * distance_tolerance(&scene),
                        "{distance} m"
                    );
                } else {
                    // A confident-looking number is only acceptable
                    // with the "too small" flag attached.
                    assert_eq!(size, TargetSize::TooSmall, "{distance} m");
                }
            }
            Err(e) => summary.push(format!("{distance} m: {e}")),
        }
    }
    eprintln!("small board sweep:\n  {}", summary.join("\n  "));
    // The farthest board has dots 3 px across: it must not be reported.
    let far = Scene::new(intrinsics_720(), 15.0, 8.0, 0.0, 6.0);
    let result = detect_board(&render(&far), &far.intrinsics);
    assert!(
        result.is_err() || result.unwrap().target_size() == TargetSize::TooSmall,
        "a 3 px dot board must be an error or flagged too small"
    );
}

#[test]
fn the_pitch_is_an_explicit_input_that_scales_the_distance() {
    // A print the printer enlarged by 1.5%: its pitch is 40.6 mm. At the
    // distance that makes it look exactly like the nominal board at
    // 1.2 m, the detector must see the same image.
    let nominal = Scene::new(intrinsics_720(), 15.0, 8.0, 0.0, 1.2);
    let mut enlarged = nominal.clone();
    enlarged.pitch_mm = 40.0 * 1.015;
    enlarged.center_m *= 1.015;
    let a = detect_board(&render(&nominal), &nominal.intrinsics).unwrap();
    let b = detect_board(&render(&enlarged), &enlarged.intrinsics).unwrap();
    assert_eq!(b.dots.len(), 34);
    assert!((a.median_dot_diameter_px - b.median_dot_diameter_px).abs() < 0.2);

    // The enlarged print really is 1.5% farther, and gives the same
    // answer in its own pitch as the nominal one does in its own.
    let da = a.pose(40.0).unwrap().distance_m;
    let db = b.pose(40.0 * 1.015).unwrap().distance_m;
    assert!((db / da - 1.015).abs() < 1e-3, "{}", db / da);
    assert!(
        (db - enlarged.true_distance()).abs() / enlarged.true_distance()
            < distance_tolerance(&enlarged)
    );

    // Telling the detector the wrong pitch moves the distance by exactly
    // that fraction: the pitch dependence is explicit.
    let right = b.pose(enlarged.pitch_mm).unwrap().distance_m;
    let wrong = b.pose(enlarged.pitch_mm * 1.01).unwrap().distance_m;
    eprintln!(
        "+1% pitch error moves the distance by {:+.6}%",
        (wrong / right - 1.0) * 100.0
    );
    assert!((wrong / right - 1.01).abs() < 1e-12);
    assert!(b.pose(0.0).is_err() && b.pose(f64::NAN).is_err() && b.pose(-40.0).is_err());
}

fn scale_frames(scene: &Scene, geometry: &CameraGeometry, bias: f64) -> Vec<Frames> {
    (0..2)
        .map(|i| frame(scene, geometry, bias, 11 + i))
        .collect()
}

/// What the ratio must be for a depth error `bias`: the depth plane is the
/// board's plane scaled about the *depth* camera, but the comparison runs
/// along the *color* camera's ray. With `D` the depth camera's distance to
/// the plane and `d` the color camera's, the plane meets that ray at
/// `1 + bias * D / d` times the true distance. Computed from the scene
/// alone, independent of the fit.
fn expected_ratio(scene: &Scene, geometry: &CameraGeometry, bias: f64) -> f64 {
    let plane = Plane::new(scene.center_m, -(scene.rotation * Vector3::z()))
        .transformed(&geometry.color_to_depth);
    let depth_origin = plane.signed_distance(&Vector3::zeros()).abs();
    let color_origin = plane
        .signed_distance(&geometry.color_to_depth.translation)
        .abs();
    1.0 + bias * depth_origin / color_origin
}

/// Half a pixel of dot-center error moves the board distance by at most
/// about 0.3% at these sizes (see `distance_tolerance`); the observed
/// errors are a few hundredths of a percent. This bound is for the whole
/// chain (board distance, depth plane, ray intersection), set an order of
/// magnitude under the 1-3% effects the test injects.
const RATIO_TOLERANCE: f64 = 0.0006;

#[test]
fn the_ratio_follows_the_depth_scale_error_on_a_flat_board() {
    let scene = Scene::new(intrinsics_720(), 0.0, 0.0, 0.0, 1.3);
    let geometry = test_geometry(scene.intrinsics);
    for bias in [0.0, 0.03, -0.012] {
        let frames = scale_frames(&scene, &geometry, bias);
        let scale = board_scale(&geometry, &frames, 40.0).expect("scale");
        eprintln!(
            "flat, bias {:+.1}%: board {:.4} m, depth {:.4} m, ratio {:.5} ({} points, {} on the plane, plane rms {:.1} mm, normals differ {:.2} deg)",
            bias * 100.0,
            scale.board_distance_m,
            scale.depth_distance_m,
            scale.ratio,
            scale.depth_points,
            scale.depth_inliers,
            scale.depth_plane_rms_m * 1000.0,
            scale.plane_disagreement_deg
        );
        assert!(
            (scale.ratio - expected_ratio(&scene, &geometry, bias)).abs() < RATIO_TOLERANCE,
            "bias {bias}: {} against {}",
            scale.ratio,
            expected_ratio(&scene, &geometry, bias)
        );
        assert!(
            (scale.board_distance_m - scene.true_distance()).abs() / scene.true_distance()
                < distance_tolerance(&scene)
        );
        assert_eq!(scale.frames_used, 2);
        assert_eq!(scale.detection.dots_matched, 34);
    }
}

#[test]
fn the_ratio_follows_the_depth_scale_error_on_a_tilted_board() {
    let scene = Scene::new(intrinsics_720(), 32.0, 14.0, 20.0, 1.4);
    let geometry = test_geometry(scene.intrinsics);
    for bias in [0.03, -0.02] {
        let frames = scale_frames(&scene, &geometry, bias);
        let scale = board_scale(&geometry, &frames, 40.0).expect("scale");
        eprintln!(
            "tilted {:.0} deg, bias {:+.1}%: board {:.4} m, depth {:.4} m, ratio {:.5} ({} points, plane rms {:.1} mm, normals differ {:.2} deg)",
            scale.view_angle_deg,
            bias * 100.0,
            scale.board_distance_m,
            scale.depth_distance_m,
            scale.ratio,
            scale.depth_points,
            scale.depth_plane_rms_m * 1000.0,
            scale.plane_disagreement_deg
        );
        assert!(
            (scale.ratio - expected_ratio(&scene, &geometry, bias)).abs() < RATIO_TOLERANCE,
            "bias {bias}: {} against {}",
            scale.ratio,
            expected_ratio(&scene, &geometry, bias)
        );
        assert!(scale.plane_disagreement_deg < 1.0);
        let distance_error = scale.board_distance_m / scene.true_distance() - 1.0;
        eprintln!("  board distance error {:+.4}%", distance_error * 100.0);
        assert!(distance_error.abs() < distance_tolerance(&scene));
    }
}

#[test]
fn the_ratio_scales_with_the_pitch_that_was_given() {
    let scene = Scene::new(intrinsics_720(), 10.0, 5.0, 0.0, 1.3);
    let geometry = test_geometry(scene.intrinsics);
    let frames = scale_frames(&scene, &geometry, 0.0);
    let right = board_scale(&geometry, &frames, 40.0).unwrap();
    let wrong = board_scale(&geometry, &frames, 40.4).unwrap();
    // A pitch 1% too large makes the board look 1% too far, so the depth
    // sensor looks 1% too near.
    assert!((wrong.board_distance_m / right.board_distance_m - 1.01).abs() < 1e-9);
    assert!((wrong.ratio * 1.01 / right.ratio - 1.0).abs() < 1e-3);
}

#[test]
fn board_scale_reports_missing_depth_and_missing_board() {
    let scene = Scene::new(intrinsics_720(), 0.0, 0.0, 0.0, 1.3);
    let geometry = test_geometry(scene.intrinsics);
    let mut frames = scale_frames(&scene, &geometry, 0.0);
    for f in &mut frames {
        f.depth = DepthImage::new(640, 400);
    }
    assert!(matches!(
        board_scale(&geometry, &frames, 40.0),
        Err(BoardError::NoDepth { .. })
    ));

    let blank = Frames {
        color: RgbImage::from_pixel(1280, 720, Rgb([200, 200, 200])),
        depth: DepthImage::new(640, 400),
        depth_unit_m: 0.001,
        ir: GrayImage::new(1, 1),
    };
    assert!(matches!(
        board_scale(&geometry, &[blank], 40.0),
        Err(BoardError::NoBoard { .. })
    ));
    assert!(matches!(
        board_scale(&geometry, &frames, -1.0),
        Err(BoardError::DegeneratePose(_))
    ));
}
