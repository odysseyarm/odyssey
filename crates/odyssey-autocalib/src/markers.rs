//! Infrared tracker markers: blob detection in the infrared image, projection
//! onto the screen plane and assignment to the expected marker layout.
//!
//! Detection runs with the camera at low exposure and the depth projector off,
//! so the markers' LEDs are the only bright features. The image is normalized
//! to its own range before thresholding, which makes the threshold relative to
//! the brightest feature and independent of the camera's absolute gain.

use image::GrayImage;
use nalgebra::Point2;

use crate::camera::CameraGeometry;
use crate::pattern::components;
use crate::plane::{apply_homography, pixel_ray};
use crate::solve::ScreenFit;

/// Marker positions as fractions of the screen's width and height, measured
/// from the top-left corner. This is the layout the tracker hardware ships
/// with; the order is the order of `ScreenCalibration::object_points`.
pub const MARKER_LAYOUT: [[f64; 2]; 6] = [
    [0.18, 0.29],
    [0.15, 0.82],
    [0.77, 0.80],
    [0.51, 0.35],
    [0.79, 0.25],
    [0.49, 0.76],
];

/// Pixels brighter than this fraction of the normalized range count as marker.
const RELATIVE_THRESHOLD: f64 = 150.0 / 255.0;
/// Frames with less contrast than this (in 8-bit levels) hold no markers.
const MIN_CONTRAST: f64 = 40.0;
const MIN_AREA_PX: u32 = 6;
const MAX_AREA_PX: u32 = 2000;
/// At most this many of the largest blobs take part in assignment, which keeps
/// the exhaustive search small when the scene is cluttered with reflections.
const MAX_BLOBS: usize = 24;
/// A blob may sit this far from its expected position, as a fraction of the
/// screen diagonal, and still be assigned to it.
const MATCH_GATE_FRACTION: f64 = 0.10;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkerBlob {
    /// Intensity-weighted centroid in the raw infrared image.
    pub center: Point2<f64>,
    pub area: u32,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MarkerError {
    #[error("found {found} bright spots, need at least 6")]
    TooFewBlobs { found: usize },
    #[error("only {matched} of 6 bright spots sit where the markers should be")]
    LayoutMismatch { matched: usize },
}

/// Finds bright round spots in an infrared image, largest first.
pub fn detect_blobs(ir: &GrayImage) -> Vec<MarkerBlob> {
    let (lo, hi) = ir
        .pixels()
        .fold((255u8, 0u8), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    if (hi as f64 - lo as f64) < MIN_CONTRAST {
        return Vec::new();
    }
    let threshold = lo as f64 + RELATIVE_THRESHOLD * (hi as f64 - lo as f64);
    let raw = ir.as_raw();
    let width = ir.width() as usize;
    let weight = |x: u32, y: u32| {
        let v = raw[y as usize * width + x as usize] as f64;
        if v >= threshold {
            v - lo as f64 + 1.0
        } else {
            0.0
        }
    };
    let mut blobs: Vec<MarkerBlob> = components(ir.width(), ir.height(), weight)
        .into_iter()
        .filter(|c| {
            (MIN_AREA_PX..=MAX_AREA_PX).contains(&c.area)
                && !c.touches_border
                && c.axis_ratio >= 0.3
        })
        .map(|c| MarkerBlob {
            center: c.center,
            area: c.area,
        })
        .collect();
    blobs.sort_by_key(|b| std::cmp::Reverse(b.area));
    blobs.truncate(MAX_BLOBS);
    blobs
}

/// Where a bright spot lies on the screen, in final screen coordinates
/// (meters), found by intersecting its infrared ray with the screen plane.
/// The infrared camera need not share the depth camera's frame.
pub fn blob_on_screen(
    geometry: &CameraGeometry,
    fit: &ScreenFit,
    blob: &MarkerBlob,
) -> Option<Point2<f64>> {
    let ray = pixel_ray(&geometry.ir, blob.center)?;
    let origin = geometry.ir_to_depth.translation;
    let direction = geometry.ir_to_depth.rotation * ray;
    let on_plane = fit.plane_depth.intersect_ray(&origin, &direction)?;
    let screen = fit.screen_from_depth.apply(&on_plane);
    apply_homography(&fit.rectification, Point2::new(screen.x, screen.y))
}

/// The six marker positions the layout predicts, in final screen coordinates.
pub fn expected_positions(fit: &ScreenFit) -> [Point2<f64>; 6] {
    MARKER_LAYOUT.map(|[u, v]| {
        apply_homography(&fit.unit_to_screen, Point2::new(u, v)).unwrap_or_else(Point2::origin)
    })
}

/// A plain-text account of where the bright spots landed on the screen against
/// where the layout expects the markers, for diagnosing a mismatch: the camera's
/// intrinsics, the screen's size and plane, every spot in the image and on the
/// screen, and the nearest spot to each expected marker.
pub fn describe_layout(geometry: &CameraGeometry, fit: &ScreenFit, blobs: &[MarkerBlob]) -> String {
    use std::fmt::Write;
    let [w, h] = fit.size_m;
    let ir = &geometry.ir;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "infrared camera {}x{} fx {:.2} fy {:.2} cx {:.2} cy {:.2}",
        ir.width, ir.height, ir.fx, ir.fy, ir.cx, ir.cy
    );
    let _ = writeln!(
        out,
        "infrared to depth translation {:?}",
        geometry.ir_to_depth.translation
    );
    let distance = fit.plane_depth.normal.dot(&fit.plane_depth.point).abs();
    let _ = writeln!(
        out,
        "screen {w:.3} x {h:.3} m, plane distance {distance:.3} m"
    );
    let expected = expected_positions(fit);
    let _ = writeln!(out, "expected markers (layout fraction -> screen meters):");
    for (i, (layout, e)) in MARKER_LAYOUT.iter().zip(&expected).enumerate() {
        let _ = writeln!(
            out,
            "  {}: ({:.2}, {:.2}) -> ({:.3}, {:.3})",
            i + 1,
            layout[0],
            layout[1],
            e.x,
            e.y
        );
    }
    let positions: Vec<Option<Point2<f64>>> = blobs
        .iter()
        .map(|b| blob_on_screen(geometry, fit, b))
        .collect();
    let _ = writeln!(
        out,
        "bright spots (infrared pixel, area -> screen meters -> fraction of the screen):"
    );
    for (i, (b, p)) in blobs.iter().zip(&positions).enumerate() {
        match p {
            Some(p) => {
                let _ = writeln!(
                    out,
                    "  {i}: ({:.1}, {:.1}) area {} -> ({:.3}, {:.3}) -> ({:.2}, {:.2})",
                    b.center.x,
                    b.center.y,
                    b.area,
                    p.x,
                    p.y,
                    p.x / w,
                    p.y / h
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "  {i}: ({:.1}, {:.1}) area {} -> misses the screen plane",
                    b.center.x, b.center.y, b.area
                );
            }
        }
    }
    let gate = MATCH_GATE_FRACTION * w.hypot(h);
    let _ = writeln!(
        out,
        "gate {gate:.3} m; nearest spot to each expected marker:"
    );
    for (i, e) in expected.iter().enumerate() {
        let nearest = positions
            .iter()
            .enumerate()
            .filter_map(|(j, p)| p.map(|p| (j, (p - e).norm())))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match nearest {
            Some((j, d)) => {
                let _ = writeln!(out, "  {}: spot {j}, {d:.3} m away", i + 1);
            }
            None => {
                let _ = writeln!(out, "  {}: no spot", i + 1);
            }
        }
    }
    out
}

/// Result of matching blobs to the layout.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerFit {
    /// Marker positions in final screen coordinates, in layout order.
    pub points_m: [Point2<f64>; 6],
    /// Index into the blob list for each layout position.
    pub blob_for_marker: [usize; 6],
    /// Largest distance between a marker and its layout position, meters.
    pub max_offset_m: f64,
}

/// Matches `blobs` to the layout and measures the markers.
pub fn locate(
    geometry: &CameraGeometry,
    fit: &ScreenFit,
    blobs: &[MarkerBlob],
) -> Result<MarkerFit, MarkerError> {
    if blobs.len() < 6 {
        return Err(MarkerError::TooFewBlobs { found: blobs.len() });
    }
    // Blobs whose ray misses the plane keep their slot so indices stay valid;
    // the matcher never selects them because they sit infinitely far away.
    let positions: Vec<Point2<f64>> = blobs
        .iter()
        .map(|b| {
            blob_on_screen(geometry, fit, b).unwrap_or(Point2::new(f64::INFINITY, f64::INFINITY))
        })
        .collect();
    let expected = expected_positions(fit);
    let gate = MATCH_GATE_FRACTION * fit.size_m[0].hypot(fit.size_m[1]);
    let Some(blob_for_marker) = assign(&expected, &positions, gate) else {
        let matched = expected
            .iter()
            .filter(|e| positions.iter().any(|p| (p - **e).norm() <= gate))
            .count();
        return Err(MarkerError::LayoutMismatch { matched });
    };
    let points_m = blob_for_marker.map(|i| positions[i]);
    let max_offset_m = points_m
        .iter()
        .zip(&expected)
        .map(|(p, e)| (p - e).norm())
        .fold(0.0, f64::max);
    Ok(MarkerFit {
        points_m,
        blob_for_marker,
        max_offset_m,
    })
}

/// Assigns each expected position a distinct observed position within `gate`
/// so that the summed distance is minimal, or `None` if no complete
/// assignment exists. The search is exhaustive over the candidates inside the
/// gate, which are few: the layout's markers are far apart relative to the
/// gate.
pub fn assign(
    expected: &[Point2<f64>; 6],
    observed: &[Point2<f64>],
    gate: f64,
) -> Option<[usize; 6]> {
    let candidates: Vec<Vec<(usize, f64)>> = expected
        .iter()
        .map(|e| {
            let mut c: Vec<_> = observed
                .iter()
                .enumerate()
                .map(|(i, o)| (i, (o - e).norm()))
                .filter(|(_, d)| *d <= gate)
                .collect();
            c.sort_by(|a, b| a.1.total_cmp(&b.1));
            c
        })
        .collect();
    if candidates.iter().any(|c| c.is_empty()) {
        return None;
    }

    struct Search<'a> {
        candidates: &'a [Vec<(usize, f64)>],
        used: Vec<bool>,
        current: [usize; 6],
        best: Option<(f64, [usize; 6])>,
    }
    impl Search<'_> {
        fn run(&mut self, slot: usize, cost: f64) {
            if self.best.as_ref().is_some_and(|(b, _)| cost >= *b) {
                return;
            }
            if slot == 6 {
                self.best = Some((cost, self.current));
                return;
            }
            for k in 0..self.candidates[slot].len() {
                let (i, d) = self.candidates[slot][k];
                if self.used[i] {
                    continue;
                }
                self.used[i] = true;
                self.current[slot] = i;
                self.run(slot + 1, cost + d);
                self.used[i] = false;
            }
        }
    }
    let mut search = Search {
        candidates: &candidates,
        used: vec![false; observed.len()],
        current: [0; 6],
        best: None,
    };
    search.run(0, 0.0);
    search.best.map(|(_, a)| a)
}

#[cfg(test)]
mod tests {
    use image::Luma;

    use super::*;

    fn layout_m(w: f64, h: f64) -> [Point2<f64>; 6] {
        MARKER_LAYOUT.map(|[u, v]| Point2::new(u * w, v * h))
    }

    #[test]
    fn assignment_ignores_clutter_and_keeps_layout_order() {
        let expected = layout_m(1.2, 0.68);
        // Measured markers are a few millimeters off, listed in shuffled order,
        // with two clutter blobs: one near a marker, one far from everything.
        let mut observed: Vec<Point2<f64>> = expected
            .iter()
            .map(|p| Point2::new(p.x + 0.003, p.y - 0.002))
            .collect();
        observed.swap(0, 4);
        observed.swap(1, 3);
        observed.push(Point2::new(expected[2].x - 0.05, expected[2].y - 0.04));
        observed.push(Point2::new(0.9, 0.05));
        let got = assign(&expected, &observed, 0.14).expect("assignment");
        let truth_order = [4usize, 3, 2, 1, 0, 5];
        assert_eq!(got, truth_order);
    }

    #[test]
    fn assignment_prefers_the_closest_marker_over_a_nearer_clutter_neighbor() {
        let expected = layout_m(1.2, 0.68);
        let mut observed: Vec<Point2<f64>> = expected.to_vec();
        // Clutter close to marker 3's slot but farther than the real marker.
        observed.push(Point2::new(expected[3].x + 0.02, expected[3].y));
        observed[3].x += 0.004;
        let got = assign(&expected, &observed, 0.14).unwrap();
        assert_eq!(got[3], 3);
    }

    #[test]
    fn assignment_rejects_when_a_marker_is_missing() {
        let expected = layout_m(1.2, 0.68);
        let observed: Vec<Point2<f64>> = expected.iter().take(5).copied().collect();
        assert!(assign(&expected, &observed, 0.14).is_none());
        // Two markers sharing one blob are not distinct inliers.
        let mut shared = expected.to_vec();
        shared[1] = shared[0];
        shared[0] = Point2::new(5.0, 5.0);
        assert!(assign(&expected, &shared, 0.14).is_none());
    }

    fn spot(img: &mut GrayImage, cx: f64, cy: f64, radius: f64, peak: u8) {
        for y in 0..img.height() {
            for x in 0..img.width() {
                let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                if d <= radius {
                    img.put_pixel(x, y, Luma([peak]));
                }
            }
        }
    }

    #[test]
    fn blob_detection_is_relative_to_the_frame_range() {
        // The same scene at two gains: dim background with dim LEDs, and a
        // brighter background with saturated LEDs.
        for (background, peak) in [(4u8, 90u8), (20, 255)] {
            let mut img = GrayImage::from_pixel(200, 150, Luma([background]));
            spot(&mut img, 50.3, 60.4, 3.0, peak);
            spot(&mut img, 150.0, 100.0, 3.0, peak);
            // A one-pixel speck as bright as the LEDs must not count.
            img.put_pixel(100, 20, Luma([peak]));
            let blobs = detect_blobs(&img);
            assert_eq!(blobs.len(), 2, "{blobs:?}");
            let near = |b: &MarkerBlob, x: f64, y: f64| {
                (b.center.x - x).abs() < 0.5 && (b.center.y - y).abs() < 0.5
            };
            assert!(blobs.iter().any(|b| near(b, 50.0, 60.0)), "{blobs:?}");
            assert!(blobs.iter().any(|b| near(b, 150.0, 100.0)), "{blobs:?}");
        }
    }

    #[test]
    fn blank_frame_has_no_blobs() {
        assert!(detect_blobs(&GrayImage::from_pixel(64, 64, Luma([30]))).is_empty());
    }
}
