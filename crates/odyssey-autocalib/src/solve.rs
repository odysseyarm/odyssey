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
//! 3. Fit the screen plane with RANSAC in the depth frame and carry it to the
//!    color frame.
//! 4. Intersect each dot's color ray with the plane for metric dot positions,
//!    rotate them into the gravity-aligned frame and express them in a frame
//!    whose `z` is the plane normal.
//! 5. Fit the homography from the pattern's unit square to those positions,
//!    rotate the in-plane axes so `x` follows the screen's top edge, and move
//!    the origin to its top-left corner.

use nalgebra::{Matrix3, Point2, Point3, Rotation3, UnitQuaternion, Vector2, Vector3};
use odyssey_common::{ScreenCalibration, ScreenCalibrationError};

use crate::camera::{CameraGeometry, Extrinsics, Frames, Intrinsics};
use crate::pattern::{Correspondence, DetectFailure, Pattern, detect};
use crate::plane::{
    HOMOGENEOUS_EPSILON, HomographyFit, Plane, PlaneFit, RansacParams, apply_homography,
    distort_pixel, fit_homography, pinhole_ray, pixel_ray, project, project_pinhole,
    ransac_homography, ransac_plane,
};

/// Color and depth frames merged per measurement. Nine frames is about
/// two-thirds of a second at the camera's 15 fps: enough for a median to reject
/// flicker and speckle without making the user hold still for long.
pub const BURST_FRAMES: usize = 9;

/// Depth readings outside this range (meters) are discarded.
pub const DEPTH_RANGE_M: (f64, f64) = (0.1, 10.0);

/// The part of the screen, in unit coordinates, whose depth feeds the plane
/// fit. Staying off the edge keeps the bezel and its shadows out.
pub const SCREEN_INSET: (f64, f64) = (0.05, 0.95);

/// Dots must agree with the dot homography to within this many meters in the
/// screen plane.
const DOT_INLIER_M: f64 = 0.006;

/// Trials the dot homography's RANSAC may spend.
const DOT_RANSAC_TRIALS: usize = 300;

/// Salt XORed into [`SolveOptions::seed`] for the screen plane's RANSAC, so
/// that the default seed `0` gives [`RansacParams`]' own default seed.
const PLANE_SEED_SALT: u64 = 0x0D155EA;

/// Salt XORed into [`SolveOptions::seed`] for the dot homography's RANSAC, so
/// the two searches draw different samples.
const DOT_SEED_SALT: u64 = 0xD075;

/// Fewest depth samples the screen plane may be fitted to, and fewest of them
/// that must lie on it.
const MIN_PLANE_POINTS: usize = 200;

/// Fewest dots that must be seen, and fewest that must agree with the dot
/// homography.
const MIN_DOTS: usize = 12;

/// Smallest cosine of the angle between the screen's normal and the ray to
/// it. `0.1` means the line of sight meets the screen at more than about
/// 6 degrees; a flatter view leaves the dot positions too ill-determined.
const MIN_PLANE_INCIDENCE_COS: f64 = 0.1;

/// Smallest norm of `gravity x normal`, which is the sine of the angle
/// between the two. Below it the screen is horizontal and has no
/// gravity-defined "across" axis.
const MIN_HORIZONTAL_ACROSS_NORM: f64 = 1e-3;

/// Meters per inch, for screen diagonals.
pub(crate) const METERS_PER_INCH: f64 = 0.0254;

/// A screen size the user or the display itself declares, used as the scale
/// anchor instead of the depth camera's measurement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnownSize {
    /// Diagonal of the displayed image in inches; the aspect ratio comes from
    /// the pattern.
    Diagonal { inches: f64 },
    /// Width and height of the displayed image in meters. Anchors both axes.
    Dimensions { width_m: f64, height_m: f64 },
}

/// How far a declared width-to-height ratio may sit from the pattern's before
/// the declaration is refused. A stretched or letterboxed desktop shows up
/// here: scaling such a size onto the pattern would silently distort the
/// screen.
pub const KNOWN_ASPECT_TOLERANCE: f64 = 0.015;

impl KnownSize {
    /// Width and height in meters for a pattern of the given aspect ratio.
    pub fn size_for_aspect(&self, aspect: f64) -> Result<[f64; 2], SolveError> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err(SolveError::KnownSizeInvalid);
        }
        match *self {
            Self::Diagonal { inches } if inches.is_finite() && inches > 0.0 => {
                let height = inches * METERS_PER_INCH / aspect.hypot(1.0);
                let width = aspect * height;
                if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                    return Err(SolveError::KnownSizeInvalid);
                }
                Ok([width, height])
            }
            Self::Dimensions { width_m, height_m }
                if width_m.is_finite()
                    && height_m.is_finite()
                    && width_m > 0.0
                    && height_m > 0.0 =>
            {
                let known_aspect = width_m / height_m;
                if (known_aspect / aspect - 1.0).abs() > KNOWN_ASPECT_TOLERANCE {
                    return Err(SolveError::KnownSizeAspectMismatch {
                        known_aspect,
                        screen_aspect: aspect,
                    });
                }
                Ok([width_m, height_m])
            }
            _ => Err(SolveError::KnownSizeInvalid),
        }
    }

    fn source(&self) -> SizeSource {
        match self {
            Self::Diagonal { .. } => SizeSource::KnownDiagonal,
            Self::Dimensions { .. } => SizeSource::KnownDimensions,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeSource {
    /// The depth camera's measurement.
    Measured,
    KnownDiagonal,
    KnownDimensions,
}

/// Relative size difference above which the depth measurement and the
/// declared size count as disagreeing. Depth scale error at room distances is
/// well under this; overscan (typically 2-5%), a wrong EDID and a wrong
/// diagonal usually exceed it.
pub const SIZE_DISAGREEMENT: f64 = 0.02;

/// The size measured from depth next to the size declared for the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeCheck {
    /// Width and height from the depth-derived fit, before rectification.
    pub measured_m: [f64; 2],
    /// Width and height that rectification imposed.
    pub known_m: [f64; 2],
    /// `measured / known` along the width.
    pub ratio_w: f64,
    /// `measured / known` along the height.
    pub ratio_h: f64,
}

impl SizeCheck {
    pub fn new(measured_m: [f64; 2], known_m: [f64; 2]) -> Self {
        Self {
            measured_m,
            known_m,
            ratio_w: measured_m[0] / known_m[0],
            ratio_h: measured_m[1] / known_m[1],
        }
    }

    /// Whether either axis differs from the declared size by more than
    /// [`SIZE_DISAGREEMENT`].
    pub fn disagrees(&self) -> bool {
        (self.ratio_w - 1.0).abs() > SIZE_DISAGREEMENT
            || (self.ratio_h - 1.0).abs() > SIZE_DISAGREEMENT
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveOptions {
    /// Declared screen size. When set, the screen is rectified in 2D to this
    /// size, so depth scale error does not affect the result.
    pub known_size: Option<KnownSize>,
    /// Seed for the plane and dot RANSAC searches; fixed so a measurement is
    /// reproducible.
    pub seed: u64,
    /// Locate dots by the perspective-corrected center instead of the raw
    /// centroid. Only turn this off to measure what the correction buys.
    pub perspective_correction: bool,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            known_size: None,
            seed: 0,
            perspective_correction: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SolveError {
    #[error("the dot pattern was not found in the color image ({0})")]
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
    #[error("the solved calibration is invalid: {0}")]
    InvalidCalibration(#[from] ScreenCalibrationError),
    #[error("the declared screen size must be positive")]
    KnownSizeInvalid,
    /// The declared width-to-height ratio differs from the displayed image's
    /// by more than [`KNOWN_ASPECT_TOLERANCE`]. Rectifying anyway would
    /// stretch the screen, so the measurement is refused.
    #[error(
        "the declared screen is {known_aspect:.3} wide per unit of height but the picture is {screen_aspect:.3}"
    )]
    KnownSizeAspectMismatch {
        known_aspect: f64,
        screen_aspect: f64,
    },
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
    /// declared size; identity without a known size.
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
    /// Where `size_m` comes from.
    pub size_source: SizeSource,
    /// The depth-measured size against the declared one; present exactly when
    /// a known size was given.
    pub size_check: Option<SizeCheck>,
    /// Maps pattern unit coordinates to undistorted color pixels, as fitted to
    /// the averaged dot centers.
    pub unit_to_pixel: Matrix3<f64>,
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
        self.size_m[0].hypot(self.size_m[1]) / METERS_PER_INCH
    }

    /// Where a point on the screen, given in final screen coordinates (meters),
    /// appears in the raw color image.
    pub fn screen_to_color_px(&self, xy: Point2<f64>) -> Option<Point2<f64>> {
        let measured = apply_homography(&self.rectification.try_inverse()?, xy)?;
        let p = self
            .screen_from_color
            .inverse()
            .apply(&Vector3::new(measured.x, measured.y, 0.0));
        project(&self.camera_color, &p)
    }
}

/// `a` after `b`.
pub fn compose(a: &Extrinsics, b: &Extrinsics) -> Extrinsics {
    Extrinsics {
        rotation: a.rotation * b.rotation,
        translation: a.rotation * b.translation + a.translation,
    }
}

/// Whether `count` is at least half of `total`: the majority rule that frames
/// and detections must pass to count as agreeing.
pub(crate) fn is_at_least_half(count: usize, total: usize) -> bool {
    count >= total.div_ceil(2)
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
    // Refuse a declaration that contradicts the picture before spending time on the burst.
    let known = options
        .known_size
        .map(|k| {
            k.size_for_aspect(pattern.aspect())
                .map(|size| (k.source(), size))
        })
        .transpose()?;
    let (dots, unit_to_pixel) =
        merged_dots(geometry, pattern, frames, options.perspective_correction)?;

    let color_from_depth = geometry.color_to_depth.inverse();
    let (plane_fit, depth_samples) = fit_screen_plane(
        geometry,
        frames,
        &unit_to_pixel,
        &color_from_depth,
        options.seed,
    )?;
    let plane_depth = plane_fit.plane;
    let plane_color = plane_depth.transformed(&color_from_depth);
    let plane_frames = PlaneFrames::new(plane_color, camera_to_aligned)?;

    let (pattern_points, plane_points) = plane_frames.dot_positions(&dots, &geometry.color);
    if pattern_points.len() < MIN_DOTS {
        return Err(SolveError::DotsInconsistent);
    }
    let dots_fit = ransac_homography(
        &pattern_points,
        &plane_points,
        DOT_INLIER_M,
        DOT_RANSAC_TRIALS,
        options.seed ^ DOT_SEED_SALT,
    )
    .filter(|fit| fit.inliers.len() >= MIN_DOTS)
    .ok_or(SolveError::DotsInconsistent)?;

    let top_edge = TopEdgeFrame::new(&dots_fit.h)?;
    let screen_from_aligned = top_edge.screen_from_aligned(&plane_frames);

    let measured_corners =
        unit_corners(&top_edge.unit_to_screen).ok_or(SolveError::DotsInconsistent)?;
    if measured_corners[1].x <= 0.0 || measured_corners[3].y <= 0.0 {
        return Err(SolveError::Mirrored);
    }
    let measured_size_m = rect_size(&measured_corners);
    let ResolvedSize {
        rectification,
        size_m,
        source: size_source,
        check: size_check,
    } = resolve_size(&measured_corners, measured_size_m, known)?;
    let unit_to_screen = rectification * top_edge.unit_to_screen;
    let corners_m = unit_corners(&unit_to_screen).ok_or(SolveError::DotsInconsistent)?;

    let screen_from_color = compose(
        &screen_from_aligned,
        &Extrinsics {
            rotation: plane_frames.alignment,
            translation: Vector3::zeros(),
        },
    );
    let screen_from_depth = compose(&screen_from_color, &color_from_depth);

    let quad_px = unit_corners(&unit_to_pixel)
        .ok_or(SolveError::DotsInconsistent)?
        .map(|corner| distort_pixel(&geometry.color, corner));

    Ok(ScreenFit {
        plane_depth,
        plane_color,
        camera_to_aligned: plane_frames.alignment,
        screen_from_aligned,
        screen_from_color,
        screen_from_depth,
        unit_to_screen,
        rectification,
        corners_m,
        measured_size_m,
        size_m,
        plane_rmse_m: plane_fit.rmse,
        plane_max_error_m: plane_fit.max_error,
        plane_inliers: plane_fit.inliers.len(),
        depth_samples,
        dots_used: dots_fit.inliers.len(),
        size_source,
        size_check,
        unit_to_pixel,
        dot_rmse_m: dot_rmse(&dots_fit, &pattern_points, &plane_points),
        quad_px,
        camera_color: geometry.color,
    })
}

/// Fits the screen plane to the depth pixels that see the screen. Returns the
/// fit and the number of depth samples it was drawn from.
fn fit_screen_plane(
    geometry: &CameraGeometry,
    frames: &[Frames],
    unit_to_pixel: &Matrix3<f64>,
    color_from_depth: &Extrinsics,
    seed: u64,
) -> Result<(PlaneFit, usize), SolveError> {
    let points = screen_depth_points(geometry, frames, unit_to_pixel, color_from_depth);
    if points.len() < MIN_PLANE_POINTS {
        return Err(SolveError::TooFewDepthPoints {
            found: points.len(),
        });
    }
    let fit = ransac_plane(
        &points,
        &RansacParams {
            seed: seed ^ PLANE_SEED_SALT,
            ..RansacParams::default()
        },
    )
    .ok_or(SolveError::NoPlane)?;
    if fit.inliers.len() < MIN_PLANE_POINTS {
        return Err(SolveError::TooFewDepthPoints {
            found: fit.inliers.len(),
        });
    }
    Ok((fit, points.len()))
}

/// The screen plane in the color frame and in the gravity-aligned frame, and
/// the rotation to coordinates whose `z` is the plane normal.
struct PlaneFrames {
    color: Plane,
    /// The gravity alignment from the camera frame.
    alignment: Matrix3<f64>,
    aligned: Plane,
    /// Takes aligned coordinates to in-plane coordinates: `z` is the plane
    /// normal, `x` is horizontal (perpendicular to gravity).
    aligned_to_plane: Matrix3<f64>,
}

impl PlaneFrames {
    fn new(color: Plane, alignment: &Matrix3<f64>) -> Result<Self, SolveError> {
        if color.normal.dot(&color.point.normalize()) < MIN_PLANE_INCIDENCE_COS {
            return Err(SolveError::DegeneratePose);
        }
        let aligned = Plane::new(alignment * color.point, alignment * color.normal);
        let z_axis = aligned.normal;
        let across = Vector3::y().cross(&z_axis);
        let x_axis = if across.norm() > MIN_HORIZONTAL_ACROSS_NORM {
            across.normalize()
        } else {
            // A horizontal screen has no gravity-defined "across"; fall back to
            // the camera's own right axis.
            (Vector3::x() - z_axis * z_axis.x).normalize()
        };
        let y_axis = z_axis.cross(&x_axis);
        Ok(Self {
            color,
            alignment: *alignment,
            aligned,
            aligned_to_plane: Matrix3::from_rows(&[
                x_axis.transpose(),
                y_axis.transpose(),
                z_axis.transpose(),
            ]),
        })
    }

    /// Each dot's pattern position paired with where its color ray meets the
    /// plane, in in-plane coordinates (meters); the two lists are parallel.
    /// Dots whose ray misses the plane are left out.
    fn dot_positions(
        &self,
        dots: &[Correspondence],
        color: &Intrinsics,
    ) -> (Vec<Point2<f64>>, Vec<Point2<f64>>) {
        let mut pattern_points = Vec::new();
        let mut plane_points = Vec::new();
        for dot in dots {
            let ray = pinhole_ray(color, dot.pixel);
            let Some(on_plane_color) = self.color.intersect_ray(&Vector3::zeros(), &ray) else {
                continue;
            };
            let p = self.aligned_to_plane * (self.alignment * on_plane_color - self.aligned.point);
            pattern_points.push(dot.pattern);
            plane_points.push(Point2::new(p.x, p.y));
        }
        (pattern_points, plane_points)
    }
}

/// In-plane axes taken from the screen's own top edge, with the origin at its
/// top-left corner.
struct TopEdgeFrame {
    /// Rotates in-plane coordinates so `x` follows the top edge.
    rotation: Matrix3<f64>,
    top_left: Point2<f64>,
    /// Maps pattern unit coordinates to meters in the screen frame, before
    /// rectification.
    unit_to_screen: Matrix3<f64>,
}

impl TopEdgeFrame {
    /// `unit_to_plane` maps pattern unit coordinates to in-plane meters.
    fn new(unit_to_plane: &Matrix3<f64>) -> Result<Self, SolveError> {
        let corner = |u, v| {
            apply_homography(unit_to_plane, Point2::new(u, v)).ok_or(SolveError::DotsInconsistent)
        };
        let (top_left, top_right) = (corner(0.0, 0.0)?, corner(1.0, 0.0)?);
        let theta = (top_right.y - top_left.y).atan2(top_right.x - top_left.x);
        let (s, c) = theta.sin_cos();
        let rotation = Matrix3::new(c, s, 0.0, -s, c, 0.0, 0.0, 0.0, 1.0);
        let shift = rotation * Vector3::new(top_left.x, top_left.y, 1.0);
        let to_screen_2d = Matrix3::new(c, s, -shift.x, -s, c, -shift.y, 0.0, 0.0, 1.0);
        Ok(Self {
            rotation,
            top_left,
            unit_to_screen: to_screen_2d * unit_to_plane,
        })
    }

    /// Takes gravity-aligned coordinates to the screen frame.
    fn screen_from_aligned(&self, plane: &PlaneFrames) -> Extrinsics {
        let rotation = self.rotation * plane.aligned_to_plane;
        let shift = self.rotation * Vector3::new(self.top_left.x, self.top_left.y, 0.0);
        Extrinsics {
            rotation,
            translation: -(rotation * plane.aligned.point) - shift,
        }
    }
}

/// The screen's final size and the rectification that imposes it.
struct ResolvedSize {
    rectification: Matrix3<f64>,
    size_m: [f64; 2],
    source: SizeSource,
    check: Option<SizeCheck>,
}

/// Rectifies the measured screen corners to the declared size, or keeps the
/// measurement when none was declared.
fn resolve_size(
    measured_corners: &[Point2<f64>; 4],
    measured_size_m: [f64; 2],
    known: Option<(SizeSource, [f64; 2])>,
) -> Result<ResolvedSize, SolveError> {
    let Some((source, [width, height])) = known else {
        return Ok(ResolvedSize {
            rectification: Matrix3::identity(),
            size_m: measured_size_m,
            source: SizeSource::Measured,
            check: None,
        });
    };
    let target = [
        Point2::new(0.0, 0.0),
        Point2::new(width, 0.0),
        Point2::new(width, height),
        Point2::new(0.0, height),
    ];
    let rectification =
        fit_homography(measured_corners, &target).ok_or(SolveError::DotsInconsistent)?;
    Ok(ResolvedSize {
        rectification,
        size_m: [width, height],
        source,
        check: Some(SizeCheck::new(measured_size_m, [width, height])),
    })
}

/// Root-mean-square distance of the homography's inlier dots from where the
/// homography puts them, meters.
fn dot_rmse(
    fit: &HomographyFit,
    pattern_points: &[Point2<f64>],
    plane_points: &[Point2<f64>],
) -> f64 {
    let sum_sq: f64 = fit
        .inliers
        .iter()
        .map(|&i| {
            (apply_homography(&fit.h, pattern_points[i]).unwrap_or(plane_points[i])
                - plane_points[i])
                .norm_squared()
        })
        .sum();
    (sum_sq / fit.inliers.len() as f64).sqrt()
}

/// Corners of the pattern's unit square, in the order of the corner arrays:
/// top left, top right, bottom right, bottom left.
const UNIT_SQUARE_CORNERS: [(f64, f64); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

fn unit_corners(h: &Matrix3<f64>) -> Option<[Point2<f64>; 4]> {
    let mut corners = [Point2::origin(); 4];
    for (slot, (u, v)) in corners.iter_mut().zip(UNIT_SQUARE_CORNERS) {
        *slot = apply_homography(h, Point2::new(u, v))?;
    }
    Some(corners)
}

/// Width and height of a quadrilateral as the mean of its opposite sides.
fn rect_size(c: &[Point2<f64>; 4]) -> [f64; 2] {
    [
        ((c[1] - c[0]).norm() + (c[2] - c[3]).norm()) / 2.0,
        ((c[3] - c[0]).norm() + (c[2] - c[1]).norm()) / 2.0,
    ]
}

/// One dot's detections summed over the burst.
#[derive(Clone, Copy)]
struct DotSum {
    /// Undistorted centers with perspective correction.
    corrected: Vector2<f64>,
    /// Undistorted centers without it.
    centroid: Vector2<f64>,
    /// Frames that saw the dot.
    frames: usize,
}

/// Detects the pattern in every frame and sums each dot's centers over the
/// frames that saw it. Returns the per-dot sums and the number of frames in
/// which the pattern was found; fails when that is under half the burst.
fn detect_burst(
    geometry: &CameraGeometry,
    pattern: &Pattern,
    frames: &[Frames],
) -> Result<(Vec<DotSum>, usize), SolveError> {
    let mut sums = vec![
        DotSum {
            corrected: Vector2::zeros(),
            centroid: Vector2::zeros(),
            frames: 0,
        };
        pattern.dots().len()
    ];
    let mut successes = 0usize;
    let mut first_failure = None;
    for frame in frames {
        match detect(pattern, &frame.color, &geometry.color) {
            Ok(detection) => {
                successes += 1;
                for c in &detection.correspondences {
                    let sum = &mut sums[c.index];
                    sum.corrected += c.pixel.coords;
                    sum.centroid += c.centroid.coords;
                    sum.frames += 1;
                }
            }
            Err(failure) => {
                first_failure.get_or_insert(failure);
            }
        }
    }
    if successes == 0 || !is_at_least_half(successes, frames.len()) {
        return Err(SolveError::PatternNotFound(
            first_failure.unwrap_or(DetectFailure::NoDots { blobs: 0 }),
        ));
    }
    Ok((sums, successes))
}

/// Detects the pattern in every frame and averages each dot's undistorted
/// center over the frames that saw it. Returns the averaged dots and the
/// homography from unit coordinates to undistorted pixels fitted to them.
fn merged_dots(
    geometry: &CameraGeometry,
    pattern: &Pattern,
    frames: &[Frames],
    perspective_correction: bool,
) -> Result<(Vec<Correspondence>, Matrix3<f64>), SolveError> {
    let (sums, successes) = detect_burst(geometry, pattern, frames)?;

    let mut dots = Vec::new();
    for (index, sum) in sums.iter().enumerate() {
        // A dot that few frames saw is more likely a misdetection than a
        // measurement.
        if sum.frames == 0 || !is_at_least_half(sum.frames, successes) {
            continue;
        }
        let dot = &pattern.dots()[index];
        let corrected = Point2::from(sum.corrected / sum.frames as f64);
        let centroid = Point2::from(sum.centroid / sum.frames as f64);
        let pixel = if perspective_correction {
            corrected
        } else {
            centroid
        };
        dots.push(Correspondence {
            index,
            pattern: Point2::new(dot.u, dot.v),
            pixel,
            centroid,
            raw: distort_pixel(&geometry.color, pixel),
        });
    }
    let grid_failure = |dots: &[Correspondence]| {
        SolveError::PatternNotFound(DetectFailure::NoGrid { blobs: dots.len() })
    };
    if dots.len() < MIN_DOTS {
        return Err(grid_failure(&dots));
    }
    let src: Vec<_> = dots.iter().map(|d| d.pattern).collect();
    let dst: Vec<_> = dots.iter().map(|d| d.pixel).collect();
    let h = fit_homography(&src, &dst).ok_or_else(|| grid_failure(&dots))?;
    Ok((dots, h))
}

/// Width of the depth image, in pixels, that the screen sampling aims at.
/// Wider images are subsampled to about this many columns, since a plane fit
/// gains nothing from the extra pixels.
const DEPTH_SAMPLE_WIDTH: u32 = 320;

/// Pixel step between depth samples for a depth image `width` pixels wide.
pub(crate) fn depth_stride(width: u32) -> u32 {
    (width / DEPTH_SAMPLE_WIDTH).max(1)
}

/// Median over the burst of the depth at pixel `(x, y)` in meters, or `None`
/// when under half the frames have a reading there. `samples` is scratch
/// space.
fn median_depth_m(frames: &[Frames], x: u32, y: u32, samples: &mut Vec<f64>) -> Option<f64> {
    samples.clear();
    samples.extend(frames.iter().filter_map(|f| f.depth_m(x, y)));
    if !is_at_least_half(samples.len(), frames.len()) {
        return None;
    }
    samples.sort_by(f64::total_cmp);
    Some(samples[(samples.len() - 1) / 2])
}

fn inside_screen_inset(uv: Point2<f64>) -> bool {
    (SCREEN_INSET.0..=SCREEN_INSET.1).contains(&uv.x)
        && (SCREEN_INSET.0..=SCREEN_INSET.1).contains(&uv.y)
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
    let Some(first) = frames.first() else {
        return Vec::new();
    };
    let Some(color_px_to_unit) = unit_to_color_px.try_inverse() else {
        return Vec::new();
    };
    let (w, h) = first.depth.dimensions();
    let stride = depth_stride(w) as usize;
    let mut samples = Vec::with_capacity(frames.len());
    let mut points = Vec::new();
    for y in (0..h).step_by(stride) {
        for x in (0..w).step_by(stride) {
            let Some(d) = median_depth_m(frames, x, y, &mut samples) else {
                continue;
            };
            if d < DEPTH_RANGE_M.0 || d > DEPTH_RANGE_M.1 {
                continue;
            }
            let Some(ray) = pixel_ray(&geometry.depth, Point2::new(f64::from(x), f64::from(y)))
            else {
                continue;
            };
            let p_depth = ray * d;
            let Some(px) = project_pinhole(&geometry.color, &color_from_depth.apply(&p_depth))
            else {
                continue;
            };
            let Some(uv) = apply_homography(&color_px_to_unit, px) else {
                continue;
            };
            if inside_screen_inset(uv) {
                points.push(p_depth);
            }
        }
    }
    points
}

/// Builds the hub's calibration from the screen fit and the six marker
/// positions, in the screen frame (meters, `z = 0`).
/// Fails instead of exporting a singular map or values that are invalid after
/// conversion to the hub's `f32` representation.
pub fn build_calibration(
    fit: &ScreenFit,
    markers: &[Point2<f64>; 6],
) -> Result<ScreenCalibration<f32>, SolveError> {
    let mut h = fit
        .unit_to_screen
        .try_inverse()
        .ok_or(SolveError::DotsInconsistent)?;
    if h[(2, 2)].abs() > HOMOGENEOUS_EPSILON {
        h /= h[(2, 2)];
    }
    let rotation = UnitQuaternion::from_rotation_matrix(&Rotation3::from_matrix_unchecked(
        fit.screen_from_aligned.rotation,
    ));
    let calibration = ScreenCalibration {
        rotation: rotation.cast(),
        homography: h.cast(),
        object_points: markers.map(|m| Point3::new(m.x as f32, m.y as f32, 0.0)),
    };
    calibration.validate()?;
    Ok(calibration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_sizes_require_a_positive_finite_aspect() {
        let diagonal = KnownSize::Diagonal { inches: 50.0 };
        let dimensions = KnownSize::Dimensions {
            width_m: 1.0,
            height_m: 1.0,
        };
        for aspect in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            for known in [diagonal, dimensions] {
                assert!(matches!(
                    known.size_for_aspect(aspect),
                    Err(SolveError::KnownSizeInvalid)
                ));
            }
        }
        // Squaring this finite aspect overflows `f64`; a naive diagonal split
        // would give a zero height.
        let size = diagonal.size_for_aspect(1e200).unwrap();
        assert!(size[0] > 0.0 && size[0].is_finite());
        assert!(size[1] > 0.0 && size[1].is_finite());
    }
}
