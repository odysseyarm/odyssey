//! Infrared tracker markers: blob detection in the infrared image, projection onto the
//! screen plane, and finding the six markers wherever they stand on or around the screen,
//! ordered the way the hub's tracker needs them (see [`crate::placement`]).
//!
//! Detection runs with the camera at low exposure and the depth projector off,
//! so the markers' LEDs are the only bright features. The image is normalized
//! to its own range before thresholding, which makes the threshold relative to
//! the brightest feature and independent of the camera's absolute gain.

use image::GrayImage;
use nalgebra::Point2;

use crate::camera::CameraGeometry;
use crate::pattern::components;
use crate::placement::{Circle, PlacementError, tracker_order};
use crate::plane::{apply_homography, pixel_ray};
use crate::solve::ScreenFit;

/// Pixels brighter than this fraction of the normalized range count as marker.
const RELATIVE_THRESHOLD: f64 = 150.0 / 255.0;
/// Frames with less contrast than this (in 8-bit levels) hold no markers.
const MIN_CONTRAST: f64 = 40.0;
const MIN_AREA_PX: u32 = 6;
const MAX_AREA_PX: u32 = 2000;
/// At most this many of the largest blobs take part in assignment, which keeps
/// the exhaustive search small when the scene is cluttered with reflections.
const MAX_BLOBS: usize = 24;
/// A spot may lie this far beyond a screen edge, as a fraction of the screen's size on that
/// axis, and still count as a marker.
const NEAR_SCREEN_MARGIN: f64 = 0.25;

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
    #[error("found {near_screen} bright spots on or near the screen, need exactly 6")]
    WrongCount { near_screen: usize },
    #[error("the six markers cannot be told apart for tracking: {0}")]
    Placement(#[from] PlacementError),
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

/// Whether `v`, a position on an axis of extent `extent`, is on the screen or within
/// [`NEAR_SCREEN_MARGIN`] of it. Markers sit on the bezel or behind the screen.
fn within(v: f64, extent: f64) -> bool {
    v.is_finite()
        && (-NEAR_SCREEN_MARGIN * extent..=(1.0 + NEAR_SCREEN_MARGIN) * extent).contains(&v)
}

/// A plain-text account of the bright spots against the screen, for diagnosing a failed
/// marker search: the camera's intrinsics, the screen's size and plane, every spot in the
/// image and on the screen (and whether it counts as on or near the screen), and the order
/// the tracker needs when the markers are found.
pub fn describe_spots(geometry: &CameraGeometry, fit: &ScreenFit, blobs: &[MarkerBlob]) -> String {
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
    let _ = writeln!(
        out,
        "bright spots (infrared pixel, area -> screen meters -> fraction of the screen):"
    );
    let mut counted = 0;
    for (i, b) in blobs.iter().enumerate() {
        match blob_on_screen(geometry, fit, b) {
            Some(p) => {
                let on = within(p.x, w) && within(p.y, h);
                counted += usize::from(on);
                let _ = writeln!(
                    out,
                    "  {i}: ({:.1}, {:.1}) area {} -> ({:.3}, {:.3}) -> ({:.2}, {:.2}){}",
                    b.center.x,
                    b.center.y,
                    b.area,
                    p.x,
                    p.y,
                    p.x / w,
                    p.y / h,
                    if on { " on or near the screen" } else { "" }
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
    let _ = writeln!(out, "{counted} spots on or near the screen, need exactly 6");
    match locate(geometry, fit, blobs) {
        Ok(m) => {
            let _ = writeln!(out, "tracker order (spot index, screen meters):");
            for (n, (i, p)) in m.blob_for_marker.iter().zip(&m.points_m).enumerate() {
                let _ = writeln!(out, "  {}: spot {i} ({:.3}, {:.3})", n + 1, p.x, p.y);
            }
            if let Some(c) = m.seed_circle {
                let _ = writeln!(
                    out,
                    "circle through markers 1, 2 and 3: centre ({:.3}, {:.3}) radius {:.3} m",
                    c.center.x, c.center.y, c.radius
                );
            }
        }
        Err(e) => {
            let _ = writeln!(out, "no marker set: {e}");
        }
    }
    out
}

/// The six markers, in the order the tracker needs them.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerFit {
    /// Marker positions in final screen coordinates, in the order of
    /// `ScreenCalibration::object_points` (see [`crate::placement`]).
    pub points_m: [Point2<f64>; 6],
    /// Index into the blob list for each marker.
    pub blob_for_marker: [usize; 6],
    /// The circle through the first three markers, the seed of the tracker's pose solve.
    pub seed_circle: Option<Circle>,
}

/// Finds the six markers among `blobs` and orders them for the tracker. The markers may
/// sit anywhere on or just around the screen: exactly six spots must be there, and three
/// of them must make the row the tracker looks for (see [`crate::placement`]).
pub fn locate(
    geometry: &CameraGeometry,
    fit: &ScreenFit,
    blobs: &[MarkerBlob],
) -> Result<MarkerFit, MarkerError> {
    if blobs.len() < 6 {
        return Err(MarkerError::TooFewBlobs { found: blobs.len() });
    }
    let [w, h] = fit.size_m;
    let on_screen: Vec<(usize, Point2<f64>)> = blobs
        .iter()
        .enumerate()
        .filter_map(|(i, b)| blob_on_screen(geometry, fit, b).map(|p| (i, p)))
        .filter(|(_, p)| within(p.x, w) && within(p.y, h))
        .collect();
    let spots: [(usize, Point2<f64>); 6] =
        on_screen
            .try_into()
            .map_err(|spots: Vec<_>| MarkerError::WrongCount {
                near_screen: spots.len(),
            })?;
    let points = spots.map(|(_, p)| p);
    let tracker = tracker_order(&points)?;
    Ok(MarkerFit {
        points_m: tracker.order.map(|n| points[n]),
        blob_for_marker: tracker.order.map(|n| spots[n].0),
        seed_circle: tracker.seed_circle,
    })
}

/// The six `points` of a later frame in the order of `reference`: each reference marker takes
/// its nearest point. The markers do not move between frames, so this labels a frame the way
/// the first one was labelled whatever order [`tracker_order`] put its own points in. `None`
/// when two reference markers claim the same point.
pub fn relabel(
    reference: &[Point2<f64>; 6],
    points: &[Point2<f64>; 6],
) -> Option<[Point2<f64>; 6]> {
    let mut taken = [false; 6];
    let mut out = [Point2::origin(); 6];
    for (slot, r) in out.iter_mut().zip(reference) {
        let (n, _) = points
            .iter()
            .enumerate()
            .map(|(n, p)| (n, (p - r).norm()))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        if std::mem::replace(&mut taken[n], true) {
            return None;
        }
        *slot = points[n];
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use image::Luma;

    use super::*;

    #[test]
    fn a_later_frame_takes_the_first_frames_labels() {
        let reference = [
            [0.2, 0.9],
            [0.2, 0.0],
            [0.8, 0.0],
            [0.5, 0.0],
            [0.5, 0.9],
            [0.8, 0.9],
        ]
        .map(|[x, y]| Point2::new(x, y));
        // The same markers a few millimeters off, listed in a different order.
        let later = [
            reference[4],
            reference[0],
            reference[5],
            reference[2],
            reference[3],
            reference[1],
        ]
        .map(|p| p + nalgebra::Vector2::new(0.003, -0.002));
        let got = relabel(&reference, &later).unwrap();
        for (g, r) in got.iter().zip(&reference) {
            assert!((g - r).norm() < 0.005);
        }
        // Two reference markers closest to the same point are not a labelling.
        let mut merged = later;
        merged[0] = merged[1];
        assert!(relabel(&reference, &merged).is_none());
    }

    fn near(p: Point2<f64>) -> bool {
        within(p.x, 1.2) && within(p.y, 0.68)
    }

    #[test]
    fn spots_on_the_bezel_or_behind_the_screen_count_but_a_lamp_beside_it_does_not() {
        // Just outside an edge: the markers on a TV's bezel.
        assert!(near(Point2::new(0.5, -0.03)));
        assert!(near(Point2::new(1.21, 0.3)));
        // Behind a projector screen.
        assert!(near(Point2::new(0.6, 0.34)));
        // A lamp well off to the side.
        assert!(!near(Point2::new(1.2 + 0.4, 0.3)));
        assert!(!near(Point2::new(0.6, -0.3)));
        assert!(!near(Point2::new(f64::INFINITY, 0.3)));
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
