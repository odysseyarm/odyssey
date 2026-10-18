//! Turns detected dots and depth into the screen's pose and size.
//!
//! Pixel coordinates follow the OpenCV convention: the center of the top-left
//! pixel is `(0, 0)`, which is also what camera SDKs report intrinsics in.
//!
//! The pipeline per measurement:
//!
//! 1. Detect the dot pattern in every color frame of a burst and average the
//!    dot centers.
//! 2. Take the per-pixel median depth over the burst, keep the pixels that
//!    land inside the inset screen region, and deproject them with the depth
//!    intrinsics.
//! 3. Fit the screen plane robustly in the depth frame and carry it to the
//!    color frame.
//! 4. Intersect each dot's color ray with the plane for metric dot positions,
//!    rotate them into the gravity-aligned frame and express them in a frame
//!    whose `z` is the plane normal.
//! 5. Fit the homography from the pattern's unit square to those positions,
//!    rotate the in-plane axes so `x` follows the screen's top edge, and move
//!    the origin to its top-left corner.

use nalgebra::{Matrix3, Point2, Point3, Rotation3, UnitQuaternion, Vector3};
use odyssey_common::ScreenCalibration;

use crate::camera::{CameraGeometry, Extrinsics, Frames, Intrinsics};
use crate::pattern::{detect, DetectFailure, Pattern};
use crate::plane::{
    apply_homography, distort_pixel, fit_homography, pixel_ray, project, project_pinhole, ransac_homography,
    ransac_plane, Plane, RansacParams,
};

/// Color and depth frames merged per measurement. Nine frames is about a third
/// of a second at 30 fps: enough for a median to reject flicker and speckle
/// without making the user hold still for long.
pub const BURST_FRAMES: usize = 9;

/// Depth readings outside this range (meters) are discarded.
pub const DEPTH_RANGE_M: (f64, f64) = (0.1, 10.0);

/// The part of the screen, in unit coordinates, whose depth feeds the plane
/// fit. Staying off the edge keeps the bezel and its shadows out.
pub const SCREEN_INSET: (f64, f64) = (0.05, 0.95);

/// Dots must agree with the dot homography to within this many meters in the
/// screen plane.
const DOT_INLIER_M: f64 = 0.006;

const MIN_PLANE_POINTS: usize = 200;
const MIN_DOTS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveOptions {
    /// Physical screen diagonal in inches. When set, the screen is rectified
    /// to this size and the pattern's aspect ratio.
    pub screen_diagonal_in: Option<f64>,
    /// Seed for the plane RANSAC; fixed so a measurement is reproducible.
    pub seed: u64,
    /// Locate dots by the perspective-corrected center instead of the raw
    /// centroid. Only turn this off to measure what the correction buys.
    pub perspective_correction: bool,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self { screen_diagonal_in: None, seed: 0, perspective_correction: true }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SolveError {
    #[error("the dot pattern was not found in the color image")]
    PatternNotFound(DetectFailure),
    #[error("only {found} depth samples lie on the screen")]
    TooFewDepthPoints { found: usize },
    #[error("no plane fits the depth measurements on the screen")]
    NoPlane,
    #[error("the screen plane is seen edge-on")]
    DegeneratePose,
    #[error("the measured screen is mirrored; the camera must look at the front of the screen")]
    Mirrored,
    #[error("dot positions do not agree with a flat screen")]
    DotsInconsistent,
}

/// Everything measured about the screen.
#[derive(Debug, Clone)]
pub struct ScreenFit {
    /// Screen plane in the depth camera's frame, normal pointing away from the camera.
    pub plane_depth: Plane,
    pub plane_color: Plane,
    pub camera_to_aligned: Matrix3<f64>,
    /// Takes gravity-aligned coordinates to the screen frame: origin at the
    /// screen's top-left corner, `x` right along the top edge, `y` down, `z`
    /// away from the viewer.
    pub screen_from_aligned: Extrinsics,
    pub screen_from_color: Extrinsics,
    pub screen_from_depth: Extrinsics,
    /// Maps pattern unit coordinates to meters in the screen frame, including
    /// the optional rectification.
    pub unit_to_screen: Matrix3<f64>,
    /// 2D projective correction that took measured screen coordinates to the
    /// requested size; identity without `screen_diagonal_in`.
    pub rectification: Matrix3<f64>,
    /// Screen corners in meters (top left, top right, bottom right, bottom
    /// left) in the screen frame.
    pub corners_m: [Point2<f64>; 4],
    /// Width and height in meters before rectification.
    pub measured_size_m: [f64; 2],
    /// Width and height in meters of the final screen model.
    pub size_m: [f64; 2],
    pub plane_rmse_m: f64,
    pub plane_max_error_m: f64,
    pub plane_inliers: usize,
    pub depth_samples: usize,
    pub dots_used: usize,
    /// Root-mean-square distance of the dots from the fitted homography, meters.
    pub dot_rmse_m: f64,
    /// Screen corners (top left, top right, bottom right, bottom left) in the
    /// raw color image.
    pub quad_px: [Point2<f64>; 4],
    /// Color intrinsics, kept so the fit can project screen points to pixels.
    pub camera_color: Intrinsics,
}

impl ScreenFit {
    pub fn diagonal_in(&self) -> f64 {
        self.size_m[0].hypot(self.size_m[1]) / 0.0254
    }

    /// Where a point on the screen, given in final screen coordinates (meters),
    /// appears in the raw color image.
    pub fn screen_to_color_px(&self, xy: Point2<f64>) -> Option<Point2<f64>> {
        let measured = apply_homography(&self.rectification.try_inverse()?, xy)?;
        let p = self.screen_from_color.inverse().apply(&Vector3::new(measured.x, measured.y, 0.0));
        project(&self.camera_color, &p)
    }
}

/// `a` after `b`.
pub fn compose(a: &Extrinsics, b: &Extrinsics) -> Extrinsics {
    Extrinsics { rotation: a.rotation * b.rotation, translation: a.rotation * b.translation + a.translation }
}

/// Measures the screen from a burst of frames.
///
/// `camera_to_aligned` is the gravity alignment from [`crate::imu`]; pass the
/// identity for a camera assumed upright.
pub fn solve_screen(
    geometry: &CameraGeometry,
    pattern: &Pattern,
    frames: &[Frames],
    camera_to_aligned: &Matrix3<f64>,
    options: &SolveOptions,
) -> Result<ScreenFit, SolveError> {
    let (dots, dots_h) = merged_dots(geometry, pattern, frames, options.perspective_correction)?;

    let color_from_depth = geometry.color_to_depth.inverse();

    let points = screen_depth_points(geometry, frames, &dots_h, &color_from_depth);
    if points.len() < MIN_PLANE_POINTS {
        return Err(SolveError::TooFewDepthPoints { found: points.len() });
    }
    let fit = ransac_plane(&points, &RansacParams { seed: options.seed ^ 0x0D155EA, ..RansacParams::default() })
        .ok_or(SolveError::NoPlane)?;
    if fit.inliers.len() < MIN_PLANE_POINTS {
        return Err(SolveError::TooFewDepthPoints { found: fit.inliers.len() });
    }
    let plane_depth = fit.plane;
    let plane_color = plane_depth.transformed(&color_from_depth);

    // Dots to metric positions on the plane, in the aligned frame.
    let a = camera_to_aligned;
    let plane_aligned = Plane::new(a * plane_color.point, a * plane_color.normal);
    let z_axis = plane_aligned.normal;
    let x_axis = {
        let across = Vector3::y().cross(&z_axis);
        if across.norm() > 1e-3 {
            across.normalize()
        } else {
            // A horizontal screen has no gravity-defined "across"; fall back to
            // the camera's own right axis.
            (Vector3::x() - z_axis * z_axis.x).normalize()
        }
    };
    let y_axis = z_axis.cross(&x_axis);
    // The ray to the plane point must meet the plane at more than ~6 degrees.
    if plane_color.normal.dot(&plane_color.point.normalize()) < 0.1 {
        return Err(SolveError::DegeneratePose);
    }
    let r_xy = Matrix3::from_rows(&[x_axis.transpose(), y_axis.transpose(), z_axis.transpose()]);

    let mut unit = Vec::new();
    let mut flat = Vec::new();
    for c in &dots {
        let ray = Vector3::new(
            (c.pixel.x - geometry.color.cx) / geometry.color.fx,
            (c.pixel.y - geometry.color.cy) / geometry.color.fy,
            1.0,
        );
        let Some(p_color) = plane_color.intersect_ray(&Vector3::zeros(), &ray) else { continue };
        let p = r_xy * (a * p_color - plane_aligned.point);
        unit.push(c.pattern);
        flat.push(Point2::new(p.x, p.y));
    }
    if unit.len() < MIN_DOTS {
        return Err(SolveError::DotsInconsistent);
    }
    let dots_fit =
        ransac_homography(&unit, &flat, DOT_INLIER_M, 300, options.seed ^ 0xD075).ok_or(SolveError::DotsInconsistent)?;
    if dots_fit.inliers.len() < MIN_DOTS {
        return Err(SolveError::DotsInconsistent);
    }
    let h_provisional = dots_fit.h;

    // Orient the in-plane axes by the screen's own top edge and put the origin
    // at its top-left corner.
    let corner = |u, v| apply_homography(&h_provisional, Point2::new(u, v)).ok_or(SolveError::DotsInconsistent);
    let (tl, tr) = (corner(0.0, 0.0)?, corner(1.0, 0.0)?);
    let theta = (tr.y - tl.y).atan2(tr.x - tl.x);
    let (s, c) = theta.sin_cos();
    let rz = Matrix3::new(c, s, 0.0, -s, c, 0.0, 0.0, 0.0, 1.0);
    let shift = rz * Vector3::new(tl.x, tl.y, 1.0);
    let to_screen_2d = Matrix3::new(c, s, -shift.x, -s, c, -shift.y, 0.0, 0.0, 1.0);
    let measured_unit_to_screen = to_screen_2d * h_provisional;

    let rot = rz * r_xy;
    let t_shift = rz * Vector3::new(tl.x, tl.y, 0.0);
    let screen_from_aligned = Extrinsics {
        rotation: rot,
        translation: -(rot * plane_aligned.point) - t_shift,
    };

    let measured_corners = unit_corners(&measured_unit_to_screen).ok_or(SolveError::DotsInconsistent)?;
    if measured_corners[1].x <= 0.0 || measured_corners[3].y <= 0.0 {
        return Err(SolveError::Mirrored);
    }
    let measured_size_m = rect_size(&measured_corners);

    let (rectification, size_m) = match options.screen_diagonal_in {
        Some(diagonal_in) if diagonal_in > 0.0 => {
            let aspect = pattern.aspect();
            let height = diagonal_in * 0.0254 / (aspect * aspect + 1.0).sqrt();
            let width = aspect * height;
            let target = [Point2::new(0.0, 0.0), Point2::new(width, 0.0), Point2::new(width, height), Point2::new(0.0, height)];
            let rect = fit_homography(&measured_corners, &target).ok_or(SolveError::DotsInconsistent)?;
            (rect, [width, height])
        }
        _ => (Matrix3::identity(), measured_size_m),
    };
    let unit_to_screen = rectification * measured_unit_to_screen;
    let corners_m = unit_corners(&unit_to_screen).ok_or(SolveError::DotsInconsistent)?;

    let screen_from_color = compose(&screen_from_aligned, &Extrinsics { rotation: *a, translation: Vector3::zeros() });
    let screen_from_depth = compose(&screen_from_color, &color_from_depth);

    let mut quad_px = [Point2::origin(); 4];
    for (slot, (u, v)) in quad_px.iter_mut().zip([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]) {
        let p = apply_homography(&dots_h, Point2::new(u, v)).ok_or(SolveError::DotsInconsistent)?;
        *slot = distort_pixel(&geometry.color, p);
    }

    let dot_sq: f64 = dots_fit
        .inliers
        .iter()
        .map(|&i| (apply_homography(&h_provisional, unit[i]).unwrap_or(flat[i]) - flat[i]).norm_squared())
        .sum();

    Ok(ScreenFit {
        plane_depth,
        plane_color,
        camera_to_aligned: *a,
        screen_from_aligned,
        screen_from_color,
        screen_from_depth,
        unit_to_screen,
        rectification,
        corners_m,
        measured_size_m,
        size_m,
        plane_rmse_m: fit.rmse,
        plane_max_error_m: fit.max_error,
        plane_inliers: fit.inliers.len(),
        depth_samples: points.len(),
        dots_used: dots_fit.inliers.len(),
        dot_rmse_m: (dot_sq / dots_fit.inliers.len() as f64).sqrt(),
        quad_px,
        camera_color: geometry.color,
    })
}

fn unit_corners(h: &Matrix3<f64>) -> Option<[Point2<f64>; 4]> {
    let m = |u, v| apply_homography(h, Point2::new(u, v));
    Some([m(0.0, 0.0)?, m(1.0, 0.0)?, m(1.0, 1.0)?, m(0.0, 1.0)?])
}

fn rect_size(c: &[Point2<f64>; 4]) -> [f64; 2] {
    [((c[1] - c[0]).norm() + (c[2] - c[3]).norm()) / 2.0, ((c[3] - c[0]).norm() + (c[2] - c[1]).norm()) / 2.0]
}

/// Detects the pattern in every frame and averages each dot's undistorted
/// center over the frames that saw it. Returns the averaged dots and the
/// homography from unit coordinates to undistorted pixels fitted to them.
fn merged_dots(
    geometry: &CameraGeometry,
    pattern: &Pattern,
    frames: &[Frames],
    perspective_correction: bool,
) -> Result<(Vec<crate::pattern::Correspondence>, Matrix3<f64>), SolveError> {
    // Per dot: summed corrected and uncorrected undistorted centers, and the
    // number of frames that saw it.
    let zero = nalgebra::Vector2::<f64>::zeros();
    let mut sums = vec![(zero, zero, 0usize); pattern.dots().len()];
    let mut successes = 0usize;
    let mut first_failure = None;
    for frame in frames {
        match detect(pattern, &frame.color, &geometry.color) {
            Ok(d) => {
                successes += 1;
                for c in &d.correspondences {
                    sums[c.index].0 += c.pixel.coords;
                    sums[c.index].1 += c.centroid.coords;
                    sums[c.index].2 += 1;
                }
            }
            Err(e) => {
                first_failure.get_or_insert(e);
            }
        }
    }
    if successes == 0 || successes * 2 < frames.len() {
        return Err(SolveError::PatternNotFound(first_failure.unwrap_or(DetectFailure::NoDots { blobs: 0 })));
    }

    let mut dots = Vec::new();
    for (index, (corrected, measured, count)) in sums.iter().enumerate() {
        if *count == 0 || *count * 2 < successes {
            continue;
        }
        let dot = &pattern.dots()[index];
        let corrected = Point2::from(corrected / *count as f64);
        let centroid = Point2::from(measured / *count as f64);
        let pixel = if perspective_correction { corrected } else { centroid };
        dots.push(crate::pattern::Correspondence {
            index,
            pattern: Point2::new(dot.u, dot.v),
            pixel,
            centroid,
            raw: distort_pixel(&geometry.color, pixel),
        });
    }
    if dots.len() < MIN_DOTS {
        return Err(SolveError::PatternNotFound(DetectFailure::NoGrid { blobs: dots.len() }));
    }
    let src: Vec<_> = dots.iter().map(|d| d.pattern).collect();
    let dst: Vec<_> = dots.iter().map(|d| d.pixel).collect();
    let h = fit_homography(&src, &dst).ok_or(SolveError::PatternNotFound(DetectFailure::NoGrid { blobs: dots.len() }))?;
    Ok((dots, h))
}

/// Depth-frame points of the pixels that see the inset screen region. Each
/// depth pixel is deprojected, moved into the color frame and located on the
/// pattern with the dot homography, which avoids searching along epipolar
/// lines and handles any color/depth baseline exactly.
pub(crate) fn screen_depth_points(
    geometry: &CameraGeometry,
    frames: &[Frames],
    unit_to_color_px: &Matrix3<f64>,
    color_from_depth: &Extrinsics,
) -> Vec<Vector3<f64>> {
    let Some(first) = frames.first() else { return Vec::new() };
    let Some(color_px_to_unit) = unit_to_color_px.try_inverse() else { return Vec::new() };
    let (w, h) = first.depth.dimensions();
    let stride = ((w / 320).max(1)) as usize;
    let needed = frames.len().div_ceil(2);
    let mut samples = Vec::with_capacity(frames.len());
    let mut points = Vec::new();
    for y in (0..h).step_by(stride) {
        for x in (0..w).step_by(stride) {
            samples.clear();
            samples.extend(frames.iter().filter_map(|f| f.depth_m(x, y)));
            if samples.len() < needed {
                continue;
            }
            samples.sort_by(f64::total_cmp);
            let d = samples[(samples.len() - 1) / 2];
            if d < DEPTH_RANGE_M.0 || d > DEPTH_RANGE_M.1 {
                continue;
            }
            let Some(ray) = pixel_ray(&geometry.depth, Point2::new(x as f64, y as f64)) else { continue };
            let p_depth = ray * d;
            let Some(px) = project_pinhole(&geometry.color, &color_from_depth.apply(&p_depth)) else { continue };
            let Some(uv) = apply_homography(&color_px_to_unit, px) else { continue };
            if (SCREEN_INSET.0..=SCREEN_INSET.1).contains(&uv.x) && (SCREEN_INSET.0..=SCREEN_INSET.1).contains(&uv.y) {
                points.push(p_depth);
            }
        }
    }
    points
}

/// Builds the hub's calibration from the screen fit and the six marker
/// positions, in the screen frame (meters, `z = 0`).
pub fn build_calibration(fit: &ScreenFit, markers: &[Point2<f64>; 6]) -> ScreenCalibration<f32> {
    let mut h = fit.unit_to_screen.try_inverse().unwrap_or_else(Matrix3::identity);
    if h[(2, 2)].abs() > 1e-12 {
        h /= h[(2, 2)];
    }
    let rotation = UnitQuaternion::from_rotation_matrix(&Rotation3::from_matrix_unchecked(fit.screen_from_aligned.rotation));
    ScreenCalibration {
        rotation: rotation.cast(),
        homography: h.cast(),
        object_points: markers.map(|m| Point3::new(m.x as f32, m.y as f32, 0.0)),
    }
}
