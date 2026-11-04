use image::Rgb;
use nalgebra::Vector3;

use super::*;
use crate::plane::{fit_homography, undistort_pixel};

fn intrinsics() -> Intrinsics {
    Intrinsics {
        width: 1280,
        height: 720,
        fx: 900.0,
        fy: 900.0,
        cx: 640.0,
        cy: 360.0,
        k: [-0.06, 0.01, 0.0, 0.0, 0.0, 0.0],
        p: [0.0004, -0.0002],
    }
}

/// Draws the pattern through a known homography with the lens model.
fn render(pattern: &Pattern, intr: &Intrinsics, h: &Matrix3<f64>) -> RgbImage {
    render_cropped(pattern, intr, h, 0.0)
}

/// Like `render`, but the panel shows only the part of the pattern image
/// that lies `crop` (a fraction of each axis) inside its edges, as an
/// overscanning display does. Outside the panel the camera sees a lit wall.
fn render_cropped(pattern: &Pattern, intr: &Intrinsics, h: &Matrix3<f64>, crop: f64) -> RgbImage {
    let mut img = RgbImage::from_pixel(intr.width, intr.height, Rgb([14, 14, 14]));
    let inv = h.try_inverse().unwrap();
    let frame = pattern.frame_rects();
    let panel = crop..=1.0 - crop;
    // Screen region is darker than the surround; fill it by inverse mapping.
    for y in 0..intr.height {
        for x in 0..intr.width {
            let Some(und) = undistort_pixel(intr, Point2::new(f64::from(x), f64::from(y))) else {
                continue;
            };
            let Some(uv) = apply_homography(&inv, und) else {
                continue;
            };
            let inside = panel.contains(&uv.x) && panel.contains(&uv.y);
            let mut value: f64 = if inside { 5.0 } else { 70.0 };
            if inside {
                let first = pattern.dots()[0];
                let (cols, rows) = pattern.grid();
                let c = (((uv.x - first.u) * pattern.aspect() / GRID_SPACING_H)
                    .round()
                    .max(0.0) as usize)
                    .min(cols - 1);
                let r =
                    (((uv.y - first.v) / GRID_SPACING_H).round().max(0.0) as usize).min(rows - 1);
                let d = pattern.dots()[r * cols + c];
                let dx = (uv.x - d.u) * pattern.aspect();
                let dy = uv.y - d.v;
                let dist = (dx * dx + dy * dy).sqrt();
                // One screen height is `scale` pixels along v.
                let scale = (apply_homography(h, Point2::new(uv.x, uv.y + 0.001)).unwrap()
                    - apply_homography(h, uv).unwrap())
                .norm()
                    / 0.001;
                let edge = (d.radius_h - dist) * scale + 0.5;
                if edge > 0.0 {
                    value = value.max(5.0 + 245.0 * edge.min(1.0));
                }
                let (px, py) = (uv.x * pattern.aspect(), uv.y);
                for rect in &frame {
                    let inset = (px - rect.x)
                        .min(rect.x + rect.width - px)
                        .min(py - rect.y)
                        .min(rect.y + rect.height - py);
                    let coverage = (inset * scale + 0.5).clamp(0.0, 1.0);
                    value = value.max(5.0 + 245.0 * coverage);
                }
            }
            img.put_pixel(x, y, Rgb([value as u8; 3]));
        }
    }
    img
}

/// Homography from the unit square to undistorted pixels for a 1.2 m wide
/// screen at depth `z`, turned about the vertical axis by `yaw` radians
/// and offset `x` meters sideways.
fn pose_homography(pattern: &Pattern, yaw: f64, x: f64, z: f64) -> Matrix3<f64> {
    let w = 1.2;
    let h_m = w / pattern.aspect();
    let r = nalgebra::Rotation3::from_euler_angles(0.0, yaw, 0.05);
    let center = Vector3::new(x, 0.0, z);
    let intr = intrinsics();
    let to_px = |u: f64, v: f64| {
        let local = Vector3::new((u - 0.5) * w, (v - 0.5) * h_m, 0.0);
        let p = r * local + center;
        Point2::new(p.x / p.z * intr.fx + intr.cx, p.y / p.z * intr.fy + intr.cy)
    };
    let src = [
        Point2::new(0.0, 0.0),
        Point2::new(1.0, 0.0),
        Point2::new(1.0, 1.0),
        Point2::new(0.0, 1.0),
    ];
    let dst = src.map(|p| to_px(p.x, p.y));
    fit_homography(&src, &dst).unwrap()
}

fn tilted_homography(pattern: &Pattern) -> Matrix3<f64> {
    pose_homography(pattern, 0.35, 0.1, 2.4)
}

#[test]
fn pattern_grid_matches_aspect() {
    let p = Pattern::new(16.0 / 9.0);
    assert_eq!(p.grid(), (16, 9));
    assert_eq!(p.dots().len(), 144);
    let anchors: Vec<_> = p
        .dots()
        .iter()
        .enumerate()
        .filter(|(_, d)| d.anchor)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(anchors, vec![0, 15, 128], "TL, TR, BL");
    assert!(
        p.dots()
            .iter()
            .all(|d| (0.0..=1.0).contains(&d.u) && (0.0..=1.0).contains(&d.v))
    );
    // Round dots: spacing in screen-height units is isotropic.
    let d0 = p.dots()[1];
    let d1 = p.dots()[0];
    assert!(((d0.u - d1.u) * p.aspect() - 0.1).abs() < 1e-9);
    assert!((p.dots()[16].v - p.dots()[0].v - 0.1).abs() < 1e-9);
    assert!(Pattern::new(1.6).grid().0 < 16);
    assert_eq!(Pattern::new(4.0 / 3.0).grid().1, 9);
}

#[test]
fn detects_a_tilted_pattern_and_orders_it_by_the_anchors() {
    let pattern = Pattern::new(16.0 / 9.0);
    let intr = intrinsics();
    let h = tilted_homography(&pattern);
    let image = render(&pattern, &intr, &h);
    let det = detect(&pattern, &image, &intr).expect("pattern found");
    assert_eq!(det.quality.matched, 144, "{:?}", det.quality);
    assert!(det.quality.rms_px < 0.3, "{:?}", det.quality);
    for c in &det.correspondences {
        let truth = apply_homography(&h, c.pattern).unwrap();
        assert!(
            (truth - c.pixel).norm() < 0.4,
            "dot {} off by {}",
            c.index,
            (truth - c.pixel).norm()
        );
    }
    let corner = apply_homography(&h, Point2::new(0.0, 0.0)).unwrap();
    let est = apply_homography(&det.homography, Point2::new(0.0, 0.0)).unwrap();
    assert!((corner - est).norm() < 1.0);
}

#[test]
fn rejects_an_image_without_a_pattern() {
    let intr = intrinsics();
    let image = RgbImage::from_pixel(intr.width, intr.height, Rgb([40, 40, 40]));
    assert!(matches!(
        detect(&Pattern::new(16.0 / 9.0), &image, &intr),
        Err(DetectFailure::NoDots { .. })
    ));
}

#[test]
fn survives_clutter_blobs_and_a_missing_dot() {
    let pattern = Pattern::new(16.0 / 9.0);
    let intr = intrinsics();
    let h = tilted_homography(&pattern);
    let mut image = render(&pattern, &intr, &h);
    // Bright round clutter off the screen and an occluded interior dot.
    for (cx, cy) in [(60i32, 60i32), (1200, 80), (100, 650)] {
        for dy in -5i32..=5 {
            for dx in -5i32..=5 {
                if dx * dx + dy * dy <= 25 {
                    image.put_pixel((cx + dx) as u32, (cy + dy) as u32, Rgb([255; 3]));
                }
            }
        }
    }
    let victim =
        apply_homography(&h, Point2::new(pattern.dots()[50].u, pattern.dots()[50].v)).unwrap();
    let victim = distort_pixel(&intr, victim);
    for dy in -12i32..=12 {
        for dx in -12i32..=12 {
            let (x, y) = ((victim.x as i32 + dx) as u32, (victim.y as i32 + dy) as u32);
            image.put_pixel(x, y, Rgb([5; 3]));
        }
    }
    let det = detect(&pattern, &image, &intr).expect("pattern found");
    assert_eq!(det.quality.matched, 143);
    assert!(det.correspondences.iter().all(|c| c.index != 50));
}

/// Area centroid of the image of a dot's outline: the polygon through the
/// projected circle, integrated numerically so it shares no algebra with
/// [`center_bias`].
fn outline_centroid(pattern: &Pattern, dot: usize, h: &Matrix3<f64>) -> Point2<f64> {
    let d = pattern.dots()[dot];
    let n = 2000;
    let ring: Vec<Point2<f64>> = (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            let u = (d.u * pattern.aspect() + d.radius_h * a.cos()) / pattern.aspect();
            apply_homography(h, Point2::new(u, d.v + d.radius_h * a.sin())).unwrap()
        })
        .collect();
    let (mut area, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (ring[i], ring[(i + 1) % n]);
        let cross = p.x * q.y - q.x * p.y;
        area += cross;
        cx += (p.x + q.x) * cross;
        cy += (p.y + q.y) * cross;
    }
    Point2::new(cx / (3.0 * area), cy / (3.0 * area))
}

#[test]
fn correction_removes_the_centroid_bias_of_noise_free_ellipses() {
    let pattern = Pattern::with_dot_scale(16.0 / 9.0, 1.4);
    let h = pose_homography(&pattern, 1.05, 0.3, 1.3);
    let measured: Vec<(usize, Point2<f64>)> = (0..pattern.dots().len())
        .map(|i| (i, outline_centroid(&pattern, i, &h)))
        .collect();
    let corrected = corrected_centers(&pattern, &h, &measured).unwrap();

    let (mut raw_max, mut fixed_max) = (0.0f64, 0.0f64);
    for ((i, centroid), fixed) in measured.iter().zip(&corrected) {
        let d = pattern.dots()[*i];
        let truth = apply_homography(&h, Point2::new(d.u, d.v)).unwrap();
        raw_max = raw_max.max((centroid - truth).norm());
        fixed_max = fixed_max.max((fixed - truth).norm());
    }
    assert!(
        raw_max > 0.3,
        "the case must have a visible bias, got {raw_max} px"
    );
    assert!(
        fixed_max < raw_max * 1e-4,
        "raw {raw_max} px, corrected {fixed_max} px"
    );
}

#[test]
fn bias_grows_with_dot_size_and_obliqueness_and_points_toward_the_near_side() {
    let pattern = Pattern::new(16.0 / 9.0);
    let big = Pattern::with_dot_scale(16.0 / 9.0, 1.4);
    let norm = |p: &Pattern, h: &Matrix3<f64>| center_bias(p, 60, h).unwrap().norm();
    let frontal = pose_homography(&pattern, 0.0, 0.0, 2.0);
    let steep = pose_homography(&pattern, 1.05, 0.0, 2.0);
    assert!(
        norm(&pattern, &frontal) < 1e-9,
        "a fronto-parallel circle has no bias"
    );
    assert!(norm(&pattern, &steep) > norm(&pattern, &pose_homography(&pattern, 0.35, 0.0, 2.0)));
    assert!(norm(&big, &steep) > 1.5 * norm(&pattern, &steep));
}

/// Mean distance of the detected dot centers from the true image of the
/// dot centers, for the corrected estimate and the raw centroid, and the number of dots matched.
fn detection_errors(pattern: &Pattern, h: &Matrix3<f64>) -> (f64, f64, f64) {
    let intr = intrinsics();
    let image = render(pattern, &intr, h);
    let det = detect(pattern, &image, &intr).expect("pattern found");
    let (mut fixed, mut raw, mut n) = (0.0, 0.0, 0.0);
    for c in &det.correspondences {
        let truth = apply_homography(h, c.pattern).unwrap();
        fixed += (c.pixel - truth).norm();
        raw += (c.centroid - truth).norm();
        n += 1.0;
    }
    (fixed / n, raw / n, n)
}

#[test]
fn corrected_centers_beat_centroids_on_a_rendered_steep_pattern() {
    let pattern = Pattern::with_dot_scale(16.0 / 9.0, 1.4);
    let h = pose_homography(&pattern, 1.05, 0.3, 1.3);
    let (fixed, raw, n) = detection_errors(&pattern, &h);
    eprintln!(
        "steep, large dots: {n} dots, mean error {raw:.3} px raw centroid, {fixed:.3} px corrected"
    );
    assert!(n >= 100.0);
    assert!(fixed < 0.5 * raw, "corrected {fixed} px vs raw {raw} px");
}

#[test]
fn frame_is_a_ring_and_brackets_that_stay_clear_of_the_dots() {
    let pattern = Pattern::new(16.0 / 9.0);
    let rects = pattern.frame_rects();
    assert_eq!(rects.len(), 12);
    for r in &rects {
        assert!(
            r.x >= 0.0
                && r.y >= 0.0
                && r.x + r.width <= pattern.aspect() + 1e-12
                && r.y + r.height <= 1.0 + 1e-12,
            "{r:?}"
        );
    }
    // No dot touches a rectangle, so the ring cannot merge with a dot blob.
    for d in pattern.dots() {
        let (cx, cy) = (d.u * pattern.aspect(), d.v);
        for r in &rects {
            let nearest = ((cx.clamp(r.x, r.x + r.width) - cx).powi(2)
                + (cy.clamp(r.y, r.y + r.height) - cy).powi(2))
            .sqrt();
            assert!(
                nearest > d.radius_h + 0.01,
                "dot at ({cx}, {cy}) is {nearest} from {r:?}"
            );
        }
    }
}

#[test]
fn ring_does_not_disturb_detection_and_reads_as_visible() {
    let pattern = Pattern::new(16.0 / 9.0);
    let intr = intrinsics();
    for (h, steep) in [
        (tilted_homography(&pattern), false),
        (pose_homography(&pattern, 0.9, 0.2, 1.6), true),
    ] {
        let image = render(&pattern, &intr, &h);
        let det = detect(&pattern, &image, &intr).expect("pattern found");
        assert_eq!(det.quality.matched, 144, "{:?}", det.quality);
        assert!(det.quality.rms_px < 0.3, "{:?}", det.quality);
        let edges = check_edges(&pattern, &image, &intr, &det.homography);
        if steep {
            // The far border is too narrow to judge, which must not read as cropped.
            assert!(!matches!(edges, EdgeCheck::Cropped { .. }), "{edges:?}");
        } else {
            assert_eq!(edges, EdgeCheck::Visible);
        }
    }
}

#[test]
fn cropped_image_is_reported_side_by_side() {
    let pattern = Pattern::new(16.0 / 9.0);
    let intr = intrinsics();
    let h = tilted_homography(&pattern);
    // 1.5% off every edge: the ring (0.6% of the height) and brackets are gone on all four sides.
    let image = render_cropped(&pattern, &intr, &h, 0.015);
    let det = detect(&pattern, &image, &intr).expect("pattern found");
    assert_eq!(
        check_edges(&pattern, &image, &intr, &det.homography),
        EdgeCheck::Cropped {
            sides: SideSet {
                top: true,
                right: true,
                bottom: true,
                left: true
            }
        }
    );
}

#[test]
fn edges_are_unknown_when_nothing_can_be_judged() {
    let pattern = Pattern::new(16.0 / 9.0);
    let intr = intrinsics();
    let h = tilted_homography(&pattern);
    // The camera sees only the middle of a pattern drawn three times as large,
    // so every ring location is outside the view.
    let zoom = Matrix3::new(3.0, 0.0, -1.0, 0.0, 3.0, -1.0, 0.0, 0.0, 1.0);
    let zoomed_h = h * zoom;
    let zoomed = render(&pattern, &intr, &zoomed_h);
    assert_eq!(
        check_edges(&pattern, &zoomed, &intr, &zoomed_h),
        EdgeCheck::Unknown
    );
    // A washed-out picture has no contrast to judge by.
    let faint = RgbImage::from_pixel(intr.width, intr.height, Rgb([30, 30, 30]));
    assert_eq!(check_edges(&pattern, &faint, &intr, &h), EdgeCheck::Unknown);
}

#[test]
fn edges_are_unknown_when_the_lens_maps_samples_to_nonfinite_pixels() {
    let pattern = Pattern::new(16.0 / 9.0);
    let mut intr = intrinsics();
    let h = pose_homography(&pattern, 0.15, 0.0, 2.2);
    let image = RgbImage::from_pixel(intr.width, intr.height, Rgb([30; 3]));
    intr.k[0] = f64::NAN;
    assert_eq!(check_edges(&pattern, &image, &intr, &h), EdgeCheck::Unknown);
}

/// A room of isolated bright spots, some bigger than the corner dots, must not
/// hide a pattern that is plainly in view: the corner dots are the ones with
/// dots beside them, not the ones that merely stand out.
#[test]
fn bright_clutter_around_the_screen_does_not_hide_the_pattern() {
    let pattern = Pattern::new(16.0 / 9.0);
    let intr = intrinsics();
    let h = pose_homography(&pattern, 0.15, 0.0, 2.2);
    let mut image = render(&pattern, &intr, &h);

    // The screen's bounding box, so the clutter stays off the screen.
    let (mut lo, mut hi) = (
        Point2::new(f64::MAX, f64::MAX),
        Point2::new(f64::MIN, f64::MIN),
    );
    for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        let p = distort_pixel(&intr, apply_homography(&h, Point2::new(u, v)).unwrap());
        lo = Point2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let margin = 25.0;

    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut placed = 0;
    while placed < 120 {
        let (cx, cy) = (next() * 1280.0, next() * 720.0);
        let radius = 3.0 + next() * 9.0;
        let near_screen = cx > lo.x - margin - radius
            && cx < hi.x + margin + radius
            && cy > lo.y - margin - radius
            && cy < hi.y + margin + radius;
        if near_screen {
            continue;
        }
        for y in (cy - radius).floor() as i32..=(cy + radius).ceil() as i32 {
            for x in (cx - radius).floor() as i32..=(cx + radius).ceil() as i32 {
                let inside = (f64::from(x) - cx).hypot(f64::from(y) - cy) <= radius;
                if inside && (0..1280).contains(&x) && (0..720).contains(&y) {
                    image.put_pixel(x as u32, y as u32, Rgb([235; 3]));
                }
            }
        }
        placed += 1;
    }

    let found = detect(&pattern, &image, &intr).expect("the pattern is in view");
    assert!(
        found.correspondences.len() * 10 >= pattern.dots().len() * 9,
        "{} of {} dots",
        found.correspondences.len(),
        pattern.dots().len()
    );
}
