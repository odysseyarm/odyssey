//! Printed scale board: an independent check of the depth sensor's scale.
//!
//! **This is a prototype for validation on real hardware.** It exists to test
//! a hypothesis: that the apparent size of a flat board of known physical size
//! tells the board's distance well enough to expose a scale error in the depth
//! sensor. Nothing here claims that accuracy; the tests only show that the
//! geometry is self-consistent on synthetic images. It does not feed any
//! calibration result.
//!
//! The board is a `cols x rows` grid of black dots on white paper
//! (`board.json`), with one dot missing to break the grid's 180-degree
//! symmetry. [`detect_board`] finds the dots in a color image and recovers the
//! board's pose relative to the camera; [`board_scale`] compares the board's
//! distance from its apparent size with the distance the depth sensor reports
//! for the same surface.
//!
//! # Units and the pitch
//!
//! Detection works in *grid units*: the board frame has its origin at the
//! center of the dot grid, `x` right and `y` down, and one unit is one dot
//! pitch. The detector therefore does not depend on the physical size of the
//! print. Everything metric takes the physical pitch in millimeters as an
//! explicit input (`pitch_mm`), because printers and media scale prints by a
//! fraction of a percent to a few percent: measure the printed scale bar with
//! a ruler and pass the corrected pitch. A pitch that is wrong by `x` percent
//! makes every board distance wrong by exactly `x` percent;
//! [`BoardSpec::nominal_pitch_mm`] is only the value `board.json` prints.
//!
//! # Definition of the distance
//!
//! [`BoardPose::distance_m`] is the Euclidean distance from the color
//! camera's optical center to the **center point of the dot grid** (the
//! midpoint between the outermost dot centers, which is also the board's
//! center because the grid is centered on the board). [`board_scale`] measures
//! the depth side with the same definition: it intersects the ray from the
//! color camera's center through that grid-center point with the plane fitted
//! to the depth points on the board, and takes the distance from the color
//! camera's center to the intersection. Both numbers are distances along the
//! same ray, so their ratio does not depend on which point of the board is
//! used, only on the plane's orientation being right.
//!
//! # What the board distance assumes
//!
//! `board_distance_m` assumes the printed dimensions (the pitch passed in) are
//! exact and the color camera's intrinsics are right. A focal length that is
//! off by 1% moves the board distance by 1%, exactly like a pitch error, and
//! the two cannot be told apart here.
//!
//! Pixel coordinates follow the rest of the crate: the center of pixel
//! `(i, j)` is the point `(i, j)`.

use std::collections::{HashSet, VecDeque};
use std::f64::consts::PI;
use std::sync::LazyLock;

use image::RgbImage;
use nalgebra::{DMatrix, DVector, Matrix2, Matrix3, Point2, Rotation3, Vector2, Vector3};
use serde::Deserialize;

use crate::camera::{CameraGeometry, Frames, Intrinsics};
use crate::plane::{
    RansacParams, apply_homography, fit_homography, project_pinhole, ransac_homography,
    ransac_plane, undistort_pixel,
};

/// Fewest matched dots for a well-conditioned fit. The full board has 34.
pub const MIN_DOTS: usize = 12;
/// Below this many matched dots a hypothesis is not even reported as "too few
/// dots": a handful of coincidentally aligned blobs is indistinguishable from
/// clutter, so the frame is simply "no board".
const MIN_PLAUSIBLE_DOTS: usize = 6;
/// Dots at least this many pixels across make a target whose center is
/// located far better than a pixel: a 1 px edge blur or demosaicing smear is
/// then under a tenth of the dot, and the grid spans about twelve dot
/// diameters, so a 0.1 px center error is below 0.1% of the grid's extent.
pub const GOOD_DOT_DIAMETER_PX: f64 = 12.0;
/// Below this the blobs are only a few pixels wide: noise, blur and the
/// detector's contrast and shape tests start to dominate, so the result
/// should not be trusted. Between the two limits the target is marginal.
pub const MIN_DOT_DIAMETER_PX: f64 = 8.0;

/// Smallest accepted blob, in pixels (a dot about four pixels across).
const MIN_BLOB_AREA_PX: usize = 12;
/// Blobs keep at most this fraction of the image.
const MAX_BLOB_AREA_FRACTION: f64 = 0.03;
/// A pixel is dark when it is under this fraction of the local white level.
const DARK_FRACTION: f32 = 0.6;
/// ...and at least this many gray levels under it.
const MIN_DARK_DROP: f32 = 25.0;
/// Local white level under this (0..255) is background, not paper.
const MIN_WHITE_LEVEL: f32 = 60.0;
/// A dot must lose at least this fraction of the paper's brightness.
const MIN_DOT_CONTRAST: f64 = 0.3;
/// Most candidate blobs the grid search looks at.
const MAX_BLOBS: usize = 120;
/// Nearest neighbors per seed blob that may serve as its grid axes.
const SEED_NEIGHBORS: usize = 6;
/// How many of the most grid-like blobs seed hypotheses.
const SEED_BLOBS: usize = 10;
/// A predicted dot matches a blob within this fraction of the local pitch.
const MATCH_TOLERANCE: f64 = 0.3;
/// The view angle (board normal against the line of sight) above which the
/// pose is rejected as degenerate.
const MAX_VIEW_ANGLE_DEG: f64 = 80.0;
/// Depth points in the board region needed before a plane is fitted.
const MIN_DEPTH_POINTS: usize = 60;
/// Inliers of the depth plane needed.
const MIN_DEPTH_INLIERS: usize = 50;
const DEPTH_RANGE_M: (f64, f64) = (0.1, 10.0);

/// Geometry of the printed board, from `board.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct BoardSpec {
    /// Printed size of the standard board, width and height in millimeters.
    pub board_mm: [f64; 2],
    pub cols: u32,
    pub rows: u32,
    /// Dot pitch of the standard print in millimeters. Nominal only: the
    /// metric functions take the pitch of the actual print as an input.
    pub pitch_mm: f64,
    pub dot_diameter_mm: f64,
    /// `[col, row]` of every absent dot.
    pub missing: Vec<[u32; 2]>,
    /// Length of the printed scale bar in millimeters.
    pub scale_bar_mm: f64,
}

static SPEC: LazyLock<BoardSpec> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../board.json"))
        .expect("board.json is embedded at build time and checked by a unit test")
});

impl BoardSpec {
    /// The board described by `board.json`.
    pub fn get() -> &'static BoardSpec {
        &SPEC
    }

    /// The pitch `board.json` prints, in millimeters. Use it as the default
    /// until the real print has been measured.
    pub fn nominal_pitch_mm() -> f64 {
        SPEC.pitch_mm
    }

    /// How many dots the board has.
    pub fn dots_expected(&self) -> usize {
        (self.cols * self.rows) as usize - self.missing.len()
    }

    /// Dot diameter in pitches.
    pub fn dot_diameter_pitches(&self) -> f64 {
        self.dot_diameter_mm / self.pitch_mm
    }

    /// Half the printed board's width and height in pitches.
    pub fn board_half_extent_pitches(&self) -> Vector2<f64> {
        Vector2::new(self.board_mm[0], self.board_mm[1]) / (2.0 * self.pitch_mm)
    }

    fn is_missing(&self, col: u32, row: u32) -> bool {
        self.missing.iter().any(|m| m[0] == col && m[1] == row)
    }

    /// Position of dot `(col, row)` in grid units: the origin is the center of
    /// the dot grid, so the grid center is `(0, 0)`.
    pub fn dot_position(&self, col: u32, row: u32) -> Vector2<f64> {
        Vector2::new(
            f64::from(col) - f64::from(self.cols - 1) / 2.0,
            f64::from(row) - f64::from(self.rows - 1) / 2.0,
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BoardError {
    #[error("no board found ({blobs} dot-like blobs, none formed the dot grid)")]
    NoBoard { blobs: usize },
    #[error("only {found} board dots matched, at least {needed} are needed")]
    TooFewDots { found: usize, needed: usize },
    #[error(
        "the board's placement is ambiguous: another placement of the grid explains the dots equally well ({matched} matched); show the corner with the missing dot"
    )]
    AmbiguousPlacement { matched: usize },
    #[error("bad fit: {0}")]
    BadFit(String),
    #[error(
        "no usable depth on the board ({points} points in the board region, {inliers} on a plane)"
    )]
    NoDepth { points: usize, inliers: usize },
    #[error("degenerate pose: {0}")]
    DegeneratePose(String),
}

/// How big the dots appear, as a rule of thumb for the target size. See
/// [`GOOD_DOT_DIAMETER_PX`] and [`MIN_DOT_DIAMETER_PX`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSize {
    TooSmall,
    Marginal,
    Adequate,
}

impl TargetSize {
    pub fn classify(dot_diameter_px: f64) -> Self {
        if dot_diameter_px >= GOOD_DOT_DIAMETER_PX {
            Self::Adequate
        } else if dot_diameter_px >= MIN_DOT_DIAMETER_PX {
            Self::Marginal
        } else {
            Self::TooSmall
        }
    }
}

/// One matched dot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchedDot {
    pub col: u32,
    pub row: u32,
    /// Estimated image of the dot's center in undistorted (pinhole) pixels,
    /// corrected for the perspective bias of the centroid.
    pub center_px: Point2<f64>,
    /// Equivalent diameter of the dot's image in pixels.
    pub diameter_px: f64,
    /// Distance of `center_px` from the fitted pose's projection, pixels.
    pub residual_px: f64,
}

/// Result of finding the board in one color image.
#[derive(Debug, Clone)]
pub struct BoardDetection {
    /// Maps grid units (origin at the grid center, `x` right, `y` down, one
    /// unit per pitch) to undistorted pixels. Built from the fitted pose.
    pub homography: Matrix3<f64>,
    pub dots: Vec<MatchedDot>,
    pub dots_expected: usize,
    /// Blobs that passed the dot tests, before grid matching.
    pub blobs_considered: usize,
    /// RMS reprojection error of the matched dots under the fitted rigid pose,
    /// in pixels.
    pub rms_px: f64,
    /// Median equivalent diameter of the matched dots' images, pixels.
    pub median_dot_diameter_px: f64,
    /// The printed board's corners (top-left, top-right, bottom-right,
    /// bottom-left) in undistorted pixels.
    pub footprint_px: [Point2<f64>; 4],
    /// Mean width and height of the printed board's image in pixels.
    pub footprint_extent_px: [f64; 2],
    /// Rotation from the board frame to the camera frame.
    pub rotation: Matrix3<f64>,
    /// Position of the grid center in the camera frame, in pitches.
    pub translation_pitches: Vector3<f64>,
}

impl BoardDetection {
    /// Whether the dots look big enough to trust, by [`TargetSize`].
    pub fn target_size(&self) -> TargetSize {
        TargetSize::classify(self.median_dot_diameter_px)
    }

    /// Metric pose for a print with the given dot pitch in millimeters.
    pub fn pose(&self, pitch_mm: f64) -> Result<BoardPose, BoardError> {
        if !(pitch_mm.is_finite() && pitch_mm > 0.0) {
            return Err(BoardError::DegeneratePose(format!(
                "pitch {pitch_mm} mm is not a positive length"
            )));
        }
        let center_m = self.translation_pitches * (pitch_mm / 1000.0);
        BoardPose::new(self.rotation, center_m, pitch_mm)
    }
}

/// Pose of the board in the color camera's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoardPose {
    /// Board frame to camera frame. The board frame has `x` right, `y` down
    /// and `z` pointing into the board, away from the viewer.
    pub rotation: Matrix3<f64>,
    /// The center point of the dot grid in the camera frame, meters.
    pub center_m: Vector3<f64>,
    /// Distance from the camera's optical center to the center point of the
    /// dot grid, meters: `center_m.norm()`.
    pub distance_m: f64,
    /// Angle between the board normal (pointing at the camera) and the line
    /// of sight to the grid center, degrees.
    pub view_angle_deg: f64,
    pub pitch_mm: f64,
}

impl BoardPose {
    fn new(
        rotation: Matrix3<f64>,
        center_m: Vector3<f64>,
        pitch_mm: f64,
    ) -> Result<Self, BoardError> {
        let distance_m = center_m.norm();
        if !distance_m.is_finite() || distance_m <= 1e-6 || center_m.z <= 0.0 {
            return Err(BoardError::DegeneratePose(
                "the board is not in front of the camera".into(),
            ));
        }
        let normal_to_camera = -(rotation * Vector3::z());
        let cos = normal_to_camera
            .dot(&(-center_m / distance_m))
            .clamp(-1.0, 1.0);
        let view_angle_deg = cos.acos().to_degrees();
        if cos <= 0.0 {
            return Err(BoardError::DegeneratePose(
                "the board faces away from the camera".into(),
            ));
        }
        if view_angle_deg > MAX_VIEW_ANGLE_DEG {
            return Err(BoardError::DegeneratePose(format!(
                "the board is seen at {view_angle_deg:.0} degrees, too oblique to measure"
            )));
        }
        Ok(Self {
            rotation,
            center_m,
            distance_m,
            view_angle_deg,
            pitch_mm,
        })
    }

    /// Board normal, pointing at the camera, in the camera frame.
    pub fn normal(&self) -> Vector3<f64> {
        -(self.rotation * Vector3::z())
    }

    /// Homography from grid units to undistorted pixels.
    pub fn grid_homography(&self, intrinsics: &Intrinsics) -> Matrix3<f64> {
        pose_homography(
            intrinsics,
            &self.rotation,
            &(self.center_m / (self.pitch_mm / 1000.0)),
        )
    }
}

fn camera_matrix(k: &Intrinsics) -> Matrix3<f64> {
    Matrix3::new(k.fx, 0.0, k.cx, 0.0, k.fy, k.cy, 0.0, 0.0, 1.0)
}

/// `K [r1 r2 t]`: grid units (plane `z = 0` of the board) to pinhole pixels.
fn pose_homography(k: &Intrinsics, r: &Matrix3<f64>, t: &Vector3<f64>) -> Matrix3<f64> {
    camera_matrix(k)
        * Matrix3::from_columns(&[r.column(0).into_owned(), r.column(1).into_owned(), *t])
}

// ---------------------------------------------------------------------------
// Blob detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct DotBlob {
    /// Undistorted (pinhole) centroid of the blob's darkness.
    center: Point2<f64>,
    diameter_px: f64,
}

fn luma(image: &RgbImage) -> Vec<f32> {
    image
        .pixels()
        .map(|p| 0.299 * f32::from(p.0[0]) + 0.587 * f32::from(p.0[1]) + 0.114 * f32::from(p.0[2]))
        .collect()
}

fn box_blur3(data: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; data.len()];
    for y in 0..h {
        let row = &data[y * w..(y + 1) * w];
        for x in 0..w {
            let (a, b) = (x.saturating_sub(1), (x + 1).min(w - 1));
            tmp[y * w + x] = (row[a] + row[x] + row[b]) / 3.0;
        }
    }
    let mut out = vec![0.0f32; data.len()];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (tmp[a * w + x] + tmp[y * w + x] + tmp[b * w + x]) / 3.0;
        }
    }
    out
}

/// Sliding-window maximum or minimum over `[i - radius, i + radius]`, clipped
/// to the line. Linear time with a monotonic deque.
fn slide_extreme(
    src: &[f32],
    radius: usize,
    max: bool,
    dst: &mut [f32],
    queue: &mut VecDeque<usize>,
) {
    let n = src.len();
    queue.clear();
    let better = |a: f32, b: f32| if max { a >= b } else { a <= b };
    let mut next = 0;
    for i in 0..n {
        let hi = (i + radius).min(n - 1);
        while next <= hi {
            while queue.back().is_some_and(|&j| better(src[next], src[j])) {
                queue.pop_back();
            }
            queue.push_back(next);
            next += 1;
        }
        while queue.front().is_some_and(|&j| j + radius < i) {
            queue.pop_front();
        }
        dst[i] = src[*queue.front().expect("the window always holds index i")];
    }
}

fn window_extreme(data: &mut [f32], w: usize, h: usize, radius: usize, max: bool) {
    let mut queue = VecDeque::new();
    let mut out = vec![0.0f32; w.max(h)];
    for y in 0..h {
        slide_extreme(
            &data[y * w..(y + 1) * w],
            radius,
            max,
            &mut out[..w],
            &mut queue,
        );
        data[y * w..(y + 1) * w].copy_from_slice(&out[..w]);
    }
    let mut column = vec![0.0f32; h];
    for x in 0..w {
        for y in 0..h {
            column[y] = data[y * w + x];
        }
        slide_extreme(&column, radius, max, &mut out[..h], &mut queue);
        for y in 0..h {
            data[y * w + x] = out[y];
        }
    }
}

/// Local white level: a morphological closing of the smoothed image, computed
/// on a reduced grid (block maximum, then dilate and erode). The closing fills
/// every dark feature smaller than its window, so dots vanish from it while
/// uneven lighting and glare survive; dark surroundings larger than the
/// window stay dark, so the reference is not dragged up there.
struct WhiteLevel {
    data: Vec<f32>,
    width: usize,
    scale: usize,
}

impl WhiteLevel {
    fn new(smooth: &[f32], w: usize, h: usize) -> Self {
        let scale = (w.min(h) / 160).max(1);
        let (cw, ch) = (w.div_ceil(scale), h.div_ceil(scale));
        let mut data = vec![0.0f32; cw * ch];
        for y in 0..h {
            for x in 0..w {
                let cell = &mut data[(y / scale) * cw + x / scale];
                *cell = cell.max(smooth[y * w + x]);
            }
        }
        // A window of a tenth of the short side on each side holds dots up to
        // a fifth of it across.
        let radius = (w.min(h) / 10 / scale).max(2);
        window_extreme(&mut data, cw, ch, radius, true);
        window_extreme(&mut data, cw, ch, radius, false);
        Self {
            data,
            width: cw,
            scale,
        }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.data[(y / self.scale) * self.width + x / self.scale]
    }
}

struct Component {
    pixels: Vec<u32>,
    min: (usize, usize),
    max: (usize, usize),
}

fn components(mask: &[bool], w: usize, h: usize) -> Vec<Component> {
    let mut seen = vec![false; mask.len()];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start as u32);
        let mut comp = Component {
            pixels: Vec::new(),
            min: (usize::MAX, usize::MAX),
            max: (0, 0),
        };
        while let Some(p) = stack.pop() {
            let (x, y) = (p as usize % w, p as usize / w);
            comp.pixels.push(p);
            comp.min = (comp.min.0.min(x), comp.min.1.min(y));
            comp.max = (comp.max.0.max(x), comp.max.1.max(y));
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let q = ny * w + nx;
                    if mask[q] && !seen[q] {
                        seen[q] = true;
                        stack.push(q as u32);
                    }
                }
            }
        }
        out.push(comp);
    }
    out
}

/// Shape test on a thresholded blob: size, border contact, ellipse-likeness
/// (the area of an ellipse with the blob's second moments must match the blob)
/// and axis ratio.
fn plausible_dot_shape(c: &Component, w: usize, h: usize) -> bool {
    let area = c.pixels.len();
    if area < MIN_BLOB_AREA_PX || area as f64 > MAX_BLOB_AREA_FRACTION * (w * h) as f64 {
        return false;
    }
    if c.min.0 <= 1 || c.min.1 <= 1 || c.max.0 + 2 >= w || c.max.1 + 2 >= h {
        return false;
    }
    let n = area as f64;
    let (mut sx, mut sy) = (0.0, 0.0);
    for &p in &c.pixels {
        sx += (p as usize % w) as f64;
        sy += (p as usize / w) as f64;
    }
    let (mx, my) = (sx / n, sy / n);
    let (mut cxx, mut cxy, mut cyy) = (0.0, 0.0, 0.0);
    for &p in &c.pixels {
        let dx = (p as usize % w) as f64 - mx;
        let dy = (p as usize / w) as f64 - my;
        cxx += dx * dx;
        cxy += dx * dy;
        cyy += dy * dy;
    }
    // One pixel of area adds 1/12 to the variance along each axis.
    let (cxx, cxy, cyy) = (cxx / n + 1.0 / 12.0, cxy / n, cyy / n + 1.0 / 12.0);
    let half_trace = (cxx + cyy) / 2.0;
    let disc = (((cxx - cyy) / 2.0).powi(2) + cxy * cxy).sqrt();
    let (l1, l2) = (half_trace + disc, half_trace - disc);
    if l2 <= 0.0 {
        return false;
    }
    let fill = n / (PI * 4.0 * (l1 * l2).sqrt());
    (0.75..=1.25).contains(&fill) && (l2 / l1).sqrt() >= 0.3
}

/// Least-squares plane `I = a + b dx + c dy` through the pixels around a dot,
/// with one round of outlier rejection. Returns the coefficients and the
/// residual standard deviation.
fn fit_white_plane(samples: &[(f64, f64, f64)]) -> Option<([f64; 3], f64)> {
    let mut active: Vec<usize> = (0..samples.len()).collect();
    let mut result = None;
    for round in 0..2 {
        if active.len() < 12 {
            return None;
        }
        let mut m = Matrix3::zeros();
        let mut b = Vector3::zeros();
        for &i in &active {
            let (dx, dy, v) = samples[i];
            let row = Vector3::new(1.0, dx, dy);
            m += row * row.transpose();
            b += row * v;
        }
        let coef = m.try_inverse()? * b;
        let residual = |s: &(f64, f64, f64)| s.2 - (coef.x + coef.y * s.0 + coef.z * s.1);
        let sigma = (active
            .iter()
            .map(|&i| residual(&samples[i]).powi(2))
            .sum::<f64>()
            / active.len() as f64)
            .sqrt();
        result = Some(([coef.x, coef.y, coef.z], sigma));
        if round == 0 {
            let keep = 2.5 * sigma.max(1.0);
            active.retain(|&i| residual(&samples[i]).abs() <= keep);
        }
    }
    result
}

/// Measures one thresholded blob against the raw image: intensity-weighted
/// centroid of the darkness relative to a locally fitted white level, and the
/// equivalent diameter. `None` when the blob's surroundings are not clean
/// paper or the contrast is too low.
fn measure_blob(
    raw: &[f32],
    mask: &[bool],
    w: usize,
    h: usize,
    c: &Component,
) -> Option<([f64; 2], f64)> {
    let grow = |m: usize| -> (usize, usize, usize, usize) {
        (
            c.min.0.saturating_sub(m),
            c.min.1.saturating_sub(m),
            (c.max.0 + m).min(w - 1),
            (c.max.1 + m).min(h - 1),
        )
    };
    let size = (c.max.0 - c.min.0 + 1).max(c.max.1 - c.min.1 + 1);
    let ring_width = 4.max(size / 4);
    let inner = grow(2);
    let outer = grow(2 + ring_width);
    let (ox, oy) = (
        (c.min.0 + c.max.0) as f64 / 2.0,
        (c.min.1 + c.max.1) as f64 / 2.0,
    );
    let mut samples = Vec::new();
    for y in outer.1..=outer.3 {
        for x in outer.0..=outer.2 {
            let inside_inner = x >= inner.0 && x <= inner.2 && y >= inner.1 && y <= inner.3;
            if !inside_inner && !mask[y * w + x] {
                samples.push((x as f64 - ox, y as f64 - oy, f64::from(raw[y * w + x])));
            }
        }
    }
    let (plane, sigma) = fit_white_plane(&samples)?;
    let white_at =
        |x: usize, y: usize| plane[0] + plane[1] * (x as f64 - ox) + plane[2] * (y as f64 - oy);

    // Intensity-weighted centroid of (white - pixel) over the blob grown by two
    // pixels, so antialiased edges count in proportion to their darkness.
    // Pixels that belong to some other dark blob are left out, so a mark just
    // outside the dot (the scale bar, a letter) cannot pull the center.
    let (bw, bh) = (inner.2 - inner.0 + 1, inner.3 - inner.1 + 1);
    let mut near = vec![false; bw * bh];
    let mut own = vec![false; bw * bh];
    for &p in &c.pixels {
        let (x, y) = (p as usize % w, p as usize / w);
        near[(y - inner.1) * bw + (x - inner.0)] = true;
        own[(y - inner.1) * bw + (x - inner.0)] = true;
    }
    for _ in 0..2 {
        let before = near.clone();
        for y in 0..bh {
            for x in 0..bw {
                if before[y * bw + x] {
                    continue;
                }
                let (y0, y1) = (y.saturating_sub(1), (y + 1).min(bh - 1));
                let (x0, x1) = (x.saturating_sub(1), (x + 1).min(bw - 1));
                near[y * bw + x] = (y0..=y1).any(|yy| (x0..=x1).any(|xx| before[yy * bw + xx]));
            }
        }
    }
    let (mut sw, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for y in inner.1..=inner.3 {
        for x in inner.0..=inner.2 {
            let local = (y - inner.1) * bw + (x - inner.0);
            if !near[local] || (mask[y * w + x] && !own[local]) {
                continue;
            }
            let weight = white_at(x, y) - f64::from(raw[y * w + x]);
            sw += weight;
            sx += weight * x as f64;
            sy += weight * y as f64;
        }
    }
    if sw <= 0.0 {
        return None;
    }
    let (cx, cy) = (sx / sw, sy / sw);
    let white = white_at(cx.round() as usize, cy.round() as usize);
    if white < f64::from(MIN_WHITE_LEVEL) || sigma > 0.15 * white {
        return None;
    }
    // Darkest fifth of the blob's pixels stands for the printed black.
    let mut levels: Vec<f32> = c.pixels.iter().map(|&p| raw[p as usize]).collect();
    levels.sort_by(f32::total_cmp);
    let darkest = (levels.len() / 5).max(1);
    let black = levels[..darkest].iter().map(|&v| f64::from(v)).sum::<f64>() / darkest as f64;
    if (white - black) / white < MIN_DOT_CONTRAST {
        return None;
    }
    let area = sw / (white - black);
    Some(([cx, cy], 2.0 * (area / PI).sqrt()))
}

fn find_dots(color: &RgbImage, intrinsics: &Intrinsics) -> Vec<DotBlob> {
    let (w, h) = (color.width() as usize, color.height() as usize);
    if w < 16 || h < 16 {
        return Vec::new();
    }
    let raw = luma(color);
    let smooth = box_blur3(&raw, w, h);
    let white = WhiteLevel::new(&smooth, w, h);
    let mut mask = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let reference = white.at(x, y);
            let v = smooth[y * w + x];
            mask[y * w + x] = reference > MIN_WHITE_LEVEL
                && v < DARK_FRACTION * reference
                && reference - v > MIN_DARK_DROP;
        }
    }

    let mut blobs: Vec<DotBlob> = components(&mask, w, h)
        .iter()
        .filter(|c| plausible_dot_shape(c, w, h))
        .filter_map(|c| {
            let ([cx, cy], diameter_px) = measure_blob(&raw, &mask, w, h, c)?;
            let center = undistort_pixel(intrinsics, Point2::new(cx, cy))?;
            Some(DotBlob {
                center,
                diameter_px,
            })
        })
        .collect();

    // Dots of one board look alike; drop blobs far from the typical size
    // (letters, ink specks, a large dark patch). Perspective changes the size
    // across the board by tens of percent, not by a factor of 2.5.
    if blobs.len() >= 4 {
        let median = median_of(blobs.iter().map(|b| b.diameter_px).collect());
        blobs.retain(|b| (0.4..=2.5).contains(&(b.diameter_px / median)));
        if blobs.len() > MAX_BLOBS {
            blobs.sort_by(|a, b| {
                (a.diameter_px / median)
                    .ln()
                    .abs()
                    .total_cmp(&(b.diameter_px / median).ln().abs())
            });
            blobs.truncate(MAX_BLOBS);
        }
    }
    blobs
}

fn median_of(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

// ---------------------------------------------------------------------------
// Grid matching
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Cell {
    col: u32,
    row: u32,
    pos: Vector2<f64>,
}

struct Lattice {
    cells: Vec<Cell>,
    cols: u32,
    rows: u32,
    index: Vec<Option<usize>>,
}

impl Lattice {
    fn new(spec: &BoardSpec) -> Self {
        let mut cells = Vec::new();
        let mut index = vec![None; (spec.cols * spec.rows) as usize];
        for row in 0..spec.rows {
            for col in 0..spec.cols {
                if spec.is_missing(col, row) {
                    continue;
                }
                index[(row * spec.cols + col) as usize] = Some(cells.len());
                cells.push(Cell {
                    col,
                    row,
                    pos: spec.dot_position(col, row),
                });
            }
        }
        Self {
            cells,
            cols: spec.cols,
            rows: spec.rows,
            index,
        }
    }

    fn at(&self, col: i64, row: i64) -> Option<usize> {
        if col < 0 || row < 0 || col >= i64::from(self.cols) || row >= i64::from(self.rows) {
            return None;
        }
        self.index[(row as usize) * self.cols as usize + col as usize]
    }
}

/// Row-major homography, for the matching loops: plain `f64` arithmetic keeps
/// the hundreds of thousands of predictions cheap even in unoptimized builds.
type Rows = [f64; 9];

fn rows_of(h: &Matrix3<f64>) -> Rows {
    [
        h[(0, 0)],
        h[(0, 1)],
        h[(0, 2)],
        h[(1, 0)],
        h[(1, 1)],
        h[(1, 2)],
        h[(2, 0)],
        h[(2, 1)],
        h[(2, 2)],
    ]
}

fn warp(h: &Rows, x: f64, y: f64) -> Option<[f64; 2]> {
    let w = h[6] * x + h[7] * y + h[8];
    (w.abs() > 1e-12).then(|| {
        [
            (h[0] * x + h[1] * y + h[2]) / w,
            (h[3] * x + h[4] * y + h[5]) / w,
        ]
    })
}

/// Image distance of one grid step at `pos` (whose image is `at`).
fn pitch_at(h: &Rows, pos: [f64; 2], at: [f64; 2]) -> Option<f64> {
    let a = warp(h, pos[0] + 1.0, pos[1])?;
    let b = warp(h, pos[0], pos[1] + 1.0)?;
    Some(((a[0] - at[0]).hypot(a[1] - at[1]) + (b[0] - at[0]).hypot(b[1] - at[1])) / 2.0)
}

#[derive(Clone)]
struct Hypothesis {
    h: Matrix3<f64>,
    /// `(blob, cell)` pairs.
    matches: Vec<(usize, usize)>,
    /// Predicted dots that fall inside the image but have no blob.
    unmatched_inside: usize,
}

type Key = (usize, std::cmp::Reverse<usize>);

impl Hypothesis {
    /// Better when more dots match, then when fewer predicted dots are
    /// missing from the visible image.
    fn key(&self) -> Key {
        (self.matches.len(), std::cmp::Reverse(self.unmatched_inside))
    }
}

/// Matches the lattice's predicted dots to blobs under `h`, each blob going
/// to the nearest prediction within [`MATCH_TOLERANCE`] of the local pitch.
/// `around` limits the prediction to cells within two grid steps of a point.
/// Returned pairs are `(blob, cell)` in blob order.
fn match_cells(
    h: &Rows,
    blobs: &[DotBlob],
    lattice: &Lattice,
    around: Option<[f64; 2]>,
) -> Vec<(usize, usize)> {
    let mut owner: Vec<Option<(f64, usize)>> = vec![None; blobs.len()];
    for (ci, cell) in lattice.cells.iter().enumerate() {
        if let Some(a) = around
            && (cell.pos.x - a[0]).abs().max((cell.pos.y - a[1]).abs()) > 2.01
        {
            continue;
        }
        let Some(px) = warp(h, cell.pos.x, cell.pos.y) else {
            continue;
        };
        let Some(pitch) = pitch_at(h, [cell.pos.x, cell.pos.y], px) else {
            continue;
        };
        let tolerance_sq = (MATCH_TOLERANCE * pitch).powi(2);
        let mut best: Option<(f64, usize)> = None;
        for (bi, blob) in blobs.iter().enumerate() {
            let d = (blob.center.x - px[0]).powi(2) + (blob.center.y - px[1]).powi(2);
            if d <= tolerance_sq && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, bi));
            }
        }
        if let Some((d, bi)) = best
            && owner[bi].is_none_or(|(od, _)| d < od)
        {
            owner[bi] = Some((d, ci));
        }
    }
    owner
        .iter()
        .enumerate()
        .filter_map(|(bi, o)| o.map(|(_, ci)| (bi, ci)))
        .collect()
}

fn fit_matches(
    matches: &[(usize, usize)],
    blobs: &[DotBlob],
    lattice: &Lattice,
) -> Option<Matrix3<f64>> {
    let src: Vec<_> = matches
        .iter()
        .map(|&(_, c)| Point2::from(lattice.cells[c].pos))
        .collect();
    let dst: Vec<_> = matches.iter().map(|&(b, _)| blobs[b].center).collect();
    fit_homography(&src, &dst)
}

/// Grows a hypothesis: matches dots under `h0`, refits a homography to the
/// matches, and repeats until the matches stop changing. The first round is
/// limited to the neighborhood of the seed because the seed is only a local
/// affine guess. `seen` holds the match sets already grown, so that two seeds
/// reaching the same placement cost one fit, not two.
fn grow_hypothesis(
    h0: &Rows,
    seed: [f64; 2],
    blobs: &[DotBlob],
    lattice: &Lattice,
    image: (f64, f64),
    seen: &mut HashSet<Vec<(usize, usize)>>,
) -> Option<Hypothesis> {
    let mut matches = match_cells(h0, blobs, lattice, Some(seed));
    if matches.len() < 5 {
        return None;
    }
    let mut h = fit_matches(&matches, blobs, lattice)?;
    for round in 0..6 {
        let next = match_cells(&rows_of(&h), blobs, lattice, None);
        if next.len() < MIN_PLAUSIBLE_DOTS {
            return None;
        }
        if round == 0 && !seen.insert(next.clone()) {
            return None;
        }
        let settled = next == matches;
        matches = next;
        if settled {
            break;
        }
        h = fit_matches(&matches, blobs, lattice)?;
    }
    let rows = rows_of(&h);
    let mut matched = vec![false; lattice.cells.len()];
    for &(_, c) in &matches {
        matched[c] = true;
    }
    let mut unmatched_inside = 0;
    for (ci, cell) in lattice.cells.iter().enumerate() {
        if matched[ci] {
            continue;
        }
        if let Some(px) = warp(&rows, cell.pos.x, cell.pos.y) {
            let margin = pitch_at(&rows, [cell.pos.x, cell.pos.y], px).map_or(0.0, |p| p / 4.0);
            if px[0] >= margin
                && px[1] >= margin
                && px[0] <= image.0 - margin
                && px[1] <= image.1 - margin
            {
                unmatched_inside += 1;
            }
        }
    }
    Some(Hypothesis {
        h,
        matches,
        unmatched_inside,
    })
}

/// Whether two placements disagree about where the grid is, by more than a
/// third of a pitch at any corner of the grid.
fn placements_differ(a: &Rows, b: &Rows, spec: &BoardSpec) -> bool {
    let half = [
        f64::from(spec.cols - 1) / 2.0,
        f64::from(spec.rows - 1) / 2.0,
    ];
    let Some(pitch) = warp(a, 0.0, 0.0).and_then(|c| pitch_at(a, [0.0, 0.0], c)) else {
        return true;
    };
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            let (x, y) = (sx * half[0], sy * half[1]);
            match (warp(a, x, y), warp(b, x, y)) {
                (Some(pa), Some(pb)) if (pa[0] - pb[0]).hypot(pa[1] - pb[1]) <= pitch / 3.0 => {}
                _ => return true,
            }
        }
    }
    false
}

/// Blobs most likely to be interior dots of a regular grid come first: the
/// ratio of the fourth to the first neighbor distance is near 1 inside a
/// lattice and large for isolated marks and for edge dots.
fn seed_order(blobs: &[DotBlob]) -> Vec<usize> {
    let mut scored: Vec<(f64, usize)> = blobs
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let mut d: Vec<f64> = blobs
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, o)| (o.center - b.center).norm())
                .collect();
            d.sort_by(f64::total_cmp);
            let score = if d.len() >= 4 {
                d[3] / d[0].max(1e-9)
            } else {
                1e6
            };
            (score, i)
        })
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored
        .into_iter()
        .take(SEED_BLOBS)
        .map(|(_, i)| i)
        .collect()
}

/// Finds the grid's placement. Each of the most grid-like blobs seeds a local
/// frame with two of its nearest neighbors (one winding only: a board seen
/// from the front is never mirrored); the frame is tried at every lattice
/// cell in the four ways a camera can see the grid rotated in its plane,
/// grown into a homography, and the hypothesis that explains the most blobs
/// wins. The missing dot is what makes the winner unique; if another
/// placement ties it, the view is ambiguous.
fn match_grid(
    blobs: &[DotBlob],
    spec: &BoardSpec,
    image: (f64, f64),
) -> Result<Hypothesis, BoardError> {
    let lattice = Lattice::new(spec);
    let mut seen = HashSet::new();
    let mut found: Vec<(Rows, Key)> = Vec::new();
    let mut best: Option<Hypothesis> = None;

    for p in seed_order(blobs) {
        let origin = blobs[p].center;
        let mut neighbors: Vec<(f64, usize)> = blobs
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != p)
            .map(|(i, b)| ((b.center - origin).norm(), i))
            .collect();
        neighbors.sort_by(|a, b| a.0.total_cmp(&b.0));
        neighbors.truncate(SEED_NEIGHBORS);
        for &(la, a) in neighbors.iter().take(2) {
            let va = blobs[a].center - origin;
            // The nearest neighbor at a clearly different direction and a
            // similar distance is the other axis.
            let other = neighbors.iter().find(|&&(lb, b)| {
                let vb = blobs[b].center - origin;
                b != a
                    && (va.x * vb.y - va.y * vb.x).abs() / (la * lb) >= 0.6
                    && (0.55..=1.8).contains(&(la / lb))
            });
            let Some(&(_, b)) = other else {
                continue;
            };
            let mut vb = blobs[b].center - origin;
            let mut va = va;
            if va.x * vb.y - va.y * vb.x < 0.0 {
                std::mem::swap(&mut va, &mut vb);
            }
            let image_axes = Matrix2::from_columns(&[va, vb]);
            for cell in &lattice.cells {
                let (c, r) = (i64::from(cell.col), i64::from(cell.row));
                for (d1, d2) in [
                    ((1, 0), (0, 1)),
                    ((0, 1), (-1, 0)),
                    ((-1, 0), (0, -1)),
                    ((0, -1), (1, 0)),
                ] {
                    if lattice.at(c + d1.0, r + d1.1).is_none()
                        || lattice.at(c + d2.0, r + d2.1).is_none()
                    {
                        continue;
                    }
                    let grid_axes =
                        Matrix2::new(d1.0 as f64, d2.0 as f64, d1.1 as f64, d2.1 as f64);
                    let affine = image_axes * grid_axes.transpose();
                    let offset = origin.coords - affine * cell.pos;
                    let h0: Rows = [
                        affine[(0, 0)],
                        affine[(0, 1)],
                        offset.x,
                        affine[(1, 0)],
                        affine[(1, 1)],
                        offset.y,
                        0.0,
                        0.0,
                        1.0,
                    ];
                    let Some(hyp) = grow_hypothesis(
                        &h0,
                        [cell.pos.x, cell.pos.y],
                        blobs,
                        &lattice,
                        image,
                        &mut seen,
                    ) else {
                        continue;
                    };
                    found.push((rows_of(&hyp.h), hyp.key()));
                    if best.as_ref().is_none_or(|b| hyp.key() > b.key()) {
                        best = Some(hyp);
                    }
                }
            }
        }
    }

    let best = best.ok_or(BoardError::NoBoard { blobs: blobs.len() })?;
    if best.matches.len() < MIN_DOTS {
        return Err(if best.matches.len() < MIN_PLAUSIBLE_DOTS {
            BoardError::NoBoard { blobs: blobs.len() }
        } else {
            BoardError::TooFewDots {
                found: best.matches.len(),
                needed: MIN_DOTS,
            }
        });
    }
    let best_rows = rows_of(&best.h);
    let best_key = best.key();
    let tied = found
        .iter()
        .any(|(h, key)| *key == best_key && placements_differ(&best_rows, h, spec));
    if tied {
        return Err(BoardError::AmbiguousPlacement {
            matched: best.matches.len(),
        });
    }
    Ok(best)
}

// ---------------------------------------------------------------------------
// Pose
// ---------------------------------------------------------------------------

/// Rotation and translation (grid units) from a grid-to-pixel homography.
fn decompose_homography(h: &Matrix3<f64>, k: &Intrinsics) -> Option<(Matrix3<f64>, Vector3<f64>)> {
    let m = camera_matrix(k).try_inverse()? * h;
    let (h1, h2, h3) = (
        m.column(0).into_owned(),
        m.column(1).into_owned(),
        m.column(2).into_owned(),
    );
    let (n1, n2) = (h1.norm(), h2.norm());
    if !(n1.is_finite() && n2.is_finite()) || n1 < 1e-12 || n2 < 1e-12 {
        return None;
    }
    let mut lambda = 2.0 / (n1 + n2);
    if h3.z * lambda < 0.0 {
        lambda = -lambda;
    }
    let (r1, r2, t) = (h1 * lambda, h2 * lambda, h3 * lambda);
    let approx = Matrix3::from_columns(&[r1, r2, r1.cross(&r2)]);
    let svd = approx.svd(true, true);
    let mut r = svd.u? * svd.v_t?;
    if r.determinant() < 0.0 {
        r = -r;
    }
    Some((r, t))
}

fn reproject(
    k: &Intrinsics,
    r: &Matrix3<f64>,
    t: &Vector3<f64>,
    g: Vector2<f64>,
) -> Option<Point2<f64>> {
    project_pinhole(k, &(r * Vector3::new(g.x, g.y, 0.0) + t))
}

fn residual_vector(
    k: &Intrinsics,
    r: &Matrix3<f64>,
    t: &Vector3<f64>,
    grid: &[Vector2<f64>],
    pixels: &[Point2<f64>],
) -> DVector<f64> {
    let mut out = DVector::zeros(2 * grid.len());
    for (i, (g, px)) in grid.iter().zip(pixels).enumerate() {
        match reproject(k, r, t, *g) {
            Some(p) => {
                out[2 * i] = p.x - px.x;
                out[2 * i + 1] = p.y - px.y;
            }
            None => {
                out[2 * i] = 1e6;
                out[2 * i + 1] = 1e6;
            }
        }
    }
    out
}

/// Levenberg-Marquardt refinement of the rigid pose against the dot centers.
/// The pose is rigid and the plane's scale is tied to the pitch, which a
/// homography decomposition does not enforce.
fn refine_pose(
    k: &Intrinsics,
    grid: &[Vector2<f64>],
    pixels: &[Point2<f64>],
    mut r: Matrix3<f64>,
    mut t: Vector3<f64>,
) -> (Matrix3<f64>, Vector3<f64>) {
    let mut cost = residual_vector(k, &r, &t, grid, pixels).norm_squared();
    let mut mu = 1e-3;
    let step = |r: &Matrix3<f64>, t: &Vector3<f64>, delta: &[f64; 6]| {
        let dr = Rotation3::from_scaled_axis(Vector3::new(delta[0], delta[1], delta[2]));
        (
            dr.into_inner() * r,
            t + Vector3::new(delta[3], delta[4], delta[5]),
        )
    };
    for _ in 0..30 {
        let base = residual_vector(k, &r, &t, grid, pixels);
        let mut jac = DMatrix::zeros(base.len(), 6);
        let eps = 1e-6;
        for j in 0..6 {
            let mut delta = [0.0; 6];
            delta[j] = eps;
            let (r2, t2) = step(&r, &t, &delta);
            let shifted = residual_vector(k, &r2, &t2, grid, pixels);
            jac.set_column(j, &((shifted - &base) / eps));
        }
        let jtj = jac.transpose() * &jac;
        let jtr = jac.transpose() * &base;
        let mut improved = false;
        for _ in 0..8 {
            let mut damped = jtj.clone();
            for d in 0..6 {
                damped[(d, d)] += mu * jtj[(d, d)].max(1e-9);
            }
            let Some(solution) = damped.lu().solve(&(-&jtr)) else {
                mu *= 10.0;
                continue;
            };
            let delta = [
                solution[0],
                solution[1],
                solution[2],
                solution[3],
                solution[4],
                solution[5],
            ];
            let (r2, t2) = step(&r, &t, &delta);
            let new_cost = residual_vector(k, &r2, &t2, grid, pixels).norm_squared();
            if new_cost < cost {
                let relative = (cost - new_cost) / cost.max(1e-30);
                r = r2;
                t = t2;
                cost = new_cost;
                mu = (mu / 5.0).max(1e-9);
                improved = true;
                if relative < 1e-12 {
                    return (r, t);
                }
                break;
            }
            mu *= 5.0;
        }
        if !improved {
            break;
        }
    }
    (r, t)
}

/// How far a dot's measured centroid sits from the image of its true center:
/// the offset between the center of the ellipse a circle of radius `rho`
/// images to and the image of the circle's center, under `h`. A circle seen at
/// an angle projects to an ellipse whose near half is bigger, so its centroid
/// is not the image of the circle's center. The ellipse center is the pole of
/// the line at infinity with respect to the imaged conic.
fn centroid_bias(h: &Matrix3<f64>, g: Vector2<f64>, rho: f64) -> Option<Vector2<f64>> {
    let inverse = h.try_inverse()?;
    let circle = Matrix3::new(
        1.0,
        0.0,
        -g.x,
        0.0,
        1.0,
        -g.y,
        -g.x,
        -g.y,
        g.x * g.x + g.y * g.y - rho * rho,
    );
    let conic = inverse.transpose() * circle * inverse;
    let pole = conic.try_inverse()? * Vector3::new(0.0, 0.0, 1.0);
    if pole.z.abs() < 1e-18 {
        return None;
    }
    let ellipse_center = Vector2::new(pole.x / pole.z, pole.y / pole.z);
    let projected = apply_homography(h, Point2::from(g))?;
    Some(ellipse_center - projected.coords)
}

/// Finds the board in a color image.
///
/// The dots are the dark, round, contrasty blobs; the local white level is a
/// morphological closing of the image, so uneven lighting, glare and dark
/// surroundings do not change which blobs count as dots. Printed text and the
/// scale bar fail the shape tests or fail to fit the grid. Blob centers are
/// intensity-weighted centroids undistorted with `intrinsics`, corrected for
/// the perspective bias of a circle's centroid once the pose is known. The
/// grid is matched without any knowledge of the print's size, so the result
/// is in grid units; ask the detection for a metric pose with
/// [`BoardDetection::pose`].
///
/// Errors: [`BoardError::NoBoard`] when no placement of the grid explains at
/// least six blobs, [`BoardError::TooFewDots`] below [`MIN_DOTS`] dots,
/// [`BoardError::AmbiguousPlacement`] when a partial view fits two placements
/// equally well, [`BoardError::BadFit`] when the matched dots do not span the
/// board or do not fit a rigid board, and [`BoardError::DegeneratePose`].
pub fn detect_board(
    color: &RgbImage,
    intrinsics: &Intrinsics,
) -> Result<BoardDetection, BoardError> {
    let spec = BoardSpec::get();
    let blobs = find_dots(color, intrinsics);
    if blobs.len() < MIN_PLAUSIBLE_DOTS {
        return Err(BoardError::NoBoard { blobs: blobs.len() });
    }
    let image = (f64::from(color.width()), f64::from(color.height()));
    let lattice = Lattice::new(spec);
    let best = match_grid(&blobs, spec, image)?;

    // Drop matches that disagree with the rest under a robust homography.
    let src: Vec<_> = best
        .matches
        .iter()
        .map(|&(_, c)| Point2::from(lattice.cells[c].pos))
        .collect();
    let dst: Vec<_> = best.matches.iter().map(|&(b, _)| blobs[b].center).collect();
    let best_rows = rows_of(&best.h);
    let pitch_px = warp(&best_rows, 0.0, 0.0)
        .and_then(|c| pitch_at(&best_rows, [0.0, 0.0], c))
        .ok_or_else(|| BoardError::BadFit("the grid homography is degenerate".into()))?;
    let fit = ransac_homography(&src, &dst, (0.05 * pitch_px).max(1.0), 300, 0xB0A2D)
        .ok_or_else(|| BoardError::BadFit("no homography fits the matched dots".into()))?;
    let kept: Vec<(usize, usize)> = fit.inliers.iter().map(|&i| best.matches[i]).collect();
    if kept.len() < MIN_DOTS {
        return Err(BoardError::TooFewDots {
            found: kept.len(),
            needed: MIN_DOTS,
        });
    }
    let mut columns: Vec<u32> = kept.iter().map(|&(_, c)| lattice.cells[c].col).collect();
    let mut rows: Vec<u32> = kept.iter().map(|&(_, c)| lattice.cells[c].row).collect();
    columns.sort_unstable();
    columns.dedup();
    rows.sort_unstable();
    rows.dedup();
    if columns.len() < 3 || rows.len() < 3 {
        return Err(BoardError::BadFit(
            "the matched dots lie along a line and do not fix the board's pose".into(),
        ));
    }

    let grid: Vec<Vector2<f64>> = kept.iter().map(|&(_, c)| lattice.cells[c].pos).collect();
    let measured: Vec<Point2<f64>> = kept.iter().map(|&(b, _)| blobs[b].center).collect();
    let rho = spec.dot_diameter_pitches() / 2.0;
    let (mut r, mut t) = decompose_homography(&fit.h, intrinsics)
        .ok_or_else(|| BoardError::DegeneratePose("the homography does not decompose".into()))?;
    if t.z <= 0.0 {
        return Err(BoardError::DegeneratePose(
            "the board is not in front of the camera".into(),
        ));
    }

    // Alternate between fitting the rigid pose and correcting each centroid
    // for the bias that pose predicts.
    let mut centers = measured.clone();
    for _ in 0..3 {
        (r, t) = refine_pose(intrinsics, &grid, &centers, r, t);
        let h = pose_homography(intrinsics, &r, &t);
        for (i, g) in grid.iter().enumerate() {
            let bias = centroid_bias(&h, *g, rho).unwrap_or_else(Vector2::zeros);
            centers[i] = measured[i] - bias;
        }
    }
    (r, t) = refine_pose(intrinsics, &grid, &centers, r, t);

    let residuals: Vec<f64> = grid
        .iter()
        .zip(&centers)
        .map(|(g, c)| reproject(intrinsics, &r, &t, *g).map_or(f64::INFINITY, |p| (p - c).norm()))
        .collect();
    let rms_px = (residuals.iter().map(|e| e * e).sum::<f64>() / residuals.len() as f64).sqrt();
    let diameters: Vec<f64> = kept.iter().map(|&(b, _)| blobs[b].diameter_px).collect();
    let median_dot_diameter_px = median_of(diameters.clone());
    if !rms_px.is_finite() || rms_px > (0.05 * median_dot_diameter_px).max(1.0) {
        return Err(BoardError::BadFit(format!(
            "reprojection error {rms_px:.2} px against dots {median_dot_diameter_px:.1} px across; the dots do not lie on a rigid flat board"
        )));
    }
    // Validates the pose (in front, facing the camera, not too oblique).
    BoardPose::new(r, t, 1.0)?;

    let homography = pose_homography(intrinsics, &r, &t);
    let half = spec.board_half_extent_pitches();
    let corner = |sx: f64, sy: f64| {
        apply_homography(&homography, Point2::new(sx * half.x, sy * half.y)).ok_or_else(|| {
            BoardError::DegeneratePose("the board's outline does not project".into())
        })
    };
    let footprint_px = [
        corner(-1.0, -1.0)?,
        corner(1.0, -1.0)?,
        corner(1.0, 1.0)?,
        corner(-1.0, 1.0)?,
    ];
    let footprint_extent_px = [
        ((footprint_px[1] - footprint_px[0]).norm() + (footprint_px[2] - footprint_px[3]).norm())
            / 2.0,
        ((footprint_px[3] - footprint_px[0]).norm() + (footprint_px[2] - footprint_px[1]).norm())
            / 2.0,
    ];
    let dots = kept
        .iter()
        .enumerate()
        .map(|(i, &(b, c))| MatchedDot {
            col: lattice.cells[c].col,
            row: lattice.cells[c].row,
            center_px: centers[i],
            diameter_px: blobs[b].diameter_px,
            residual_px: residuals[i],
        })
        .collect();
    Ok(BoardDetection {
        homography,
        dots,
        dots_expected: spec.dots_expected(),
        blobs_considered: blobs.len(),
        rms_px,
        median_dot_diameter_px,
        footprint_px,
        footprint_extent_px,
        rotation: r,
        translation_pitches: t,
    })
}

// ---------------------------------------------------------------------------
// Scale comparison
// ---------------------------------------------------------------------------

/// Detection statistics over the frames that found the board (medians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectionStats {
    pub dots_matched: usize,
    pub dots_expected: usize,
    pub median_dot_diameter_px: f64,
    /// Width and height of the printed board's image, pixels.
    pub footprint_extent_px: [f64; 2],
    pub rms_px: f64,
    pub target_size: TargetSize,
}

/// The board's distance from its apparent size next to the depth sensor's
/// distance to the same surface.
#[derive(Debug, Clone)]
pub struct BoardScale {
    /// Pitch the board distance was computed with, millimeters.
    pub pitch_mm: f64,
    /// Median distance to the grid center from the board's apparent size,
    /// meters. Assumes the pitch is exact and the color intrinsics are right.
    pub board_distance_m: f64,
    /// Distance to the same point from the depth plane, meters.
    pub depth_distance_m: f64,
    /// `depth_distance_m / board_distance_m`. 1.0 means the depth sensor
    /// agrees; 1.03 means it reads 3% too far.
    pub ratio: f64,
    /// Frames in which the board was found, and frames supplied.
    pub frames_used: usize,
    pub frames_total: usize,
    /// Board distance of every frame that found the board, meters.
    pub frame_distances_m: Vec<f64>,
    /// Depth points in the board region, and those on the fitted plane.
    pub depth_points: usize,
    pub depth_inliers: usize,
    /// RMS distance of the plane's inliers from it, meters.
    pub depth_plane_rms_m: f64,
    /// Angle between the depth plane's normal and the board's, degrees. Large
    /// values mean the two measurements are not looking at the same surface.
    pub plane_disagreement_deg: f64,
    /// Angle between the board normal and the line of sight, degrees.
    pub view_angle_deg: f64,
    pub detection: DetectionStats,
}

impl BoardScale {
    /// Sample standard deviation of the per-frame board distance, meters.
    pub fn distance_spread_m(&self) -> f64 {
        let n = self.frame_distances_m.len();
        if n < 2 {
            return 0.0;
        }
        let mean = self.frame_distances_m.iter().sum::<f64>() / n as f64;
        (self
            .frame_distances_m
            .iter()
            .map(|d| (d - mean).powi(2))
            .sum::<f64>()
            / (n - 1) as f64)
            .sqrt()
    }
}

/// Depth-frame points of the pixels that see the dot grid's region (the grid's
/// extent plus one dot radius), found by moving each deprojected depth pixel
/// into the color frame and locating it on the board with the homography.
fn board_depth_points(
    geometry: &CameraGeometry,
    frames: &[Frames],
    grid_to_px: &Matrix3<f64>,
) -> Vec<Vector3<f64>> {
    let spec = BoardSpec::get();
    let Some(first) = frames.first() else {
        return Vec::new();
    };
    let Some(px_to_grid) = grid_to_px.try_inverse() else {
        return Vec::new();
    };
    let color_from_depth = geometry.color_to_depth.inverse();
    let rho = spec.dot_diameter_pitches() / 2.0;
    let half = Vector2::new(f64::from(spec.cols - 1), f64::from(spec.rows - 1)) / 2.0
        + Vector2::new(rho, rho);
    let (w, h) = first.depth.dimensions();
    let usable: Vec<&Frames> = frames
        .iter()
        .filter(|f| f.depth.dimensions() == (w, h))
        .collect();
    let needed = usable.len().div_ceil(2);
    let mut samples = Vec::with_capacity(usable.len());
    let mut points = Vec::new();
    for y in 0..h {
        for x in 0..w {
            samples.clear();
            samples.extend(usable.iter().filter_map(|f| f.depth_m(x, y)));
            if samples.len() < needed.max(1) {
                continue;
            }
            samples.sort_by(f64::total_cmp);
            let mid = samples.len() / 2;
            let d = if samples.len() % 2 == 1 {
                samples[mid]
            } else {
                (samples[mid - 1] + samples[mid]) / 2.0
            };
            if d < DEPTH_RANGE_M.0 || d > DEPTH_RANGE_M.1 {
                continue;
            }
            let Some(ray) =
                crate::plane::pixel_ray(&geometry.depth, Point2::new(f64::from(x), f64::from(y)))
            else {
                continue;
            };
            let p_depth = ray * d;
            let Some(px) = project_pinhole(&geometry.color, &color_from_depth.apply(&p_depth))
            else {
                continue;
            };
            let Some(g) = apply_homography(&px_to_grid, px) else {
                continue;
            };
            if g.x.abs() <= half.x && g.y.abs() <= half.y {
                points.push(p_depth);
            }
        }
    }
    points
}

/// Compares the board's apparent distance with the depth sensor's.
///
/// The board is detected in each frame's color image; the median distance
/// over the frames that find it is `board_distance_m`. The depth points that
/// fall on the dot grid's region (mapped from depth to color with
/// `geometry.color_to_depth`) are medianed over the frames and fitted with a
/// RANSAC plane, which rejects outliers and depth dropouts. `depth_distance_m`
/// is the distance from the color camera's center to where that plane meets
/// the ray through the grid center, the same point `board_distance_m` refers
/// to; see the module documentation for the exact definition.
///
/// `pitch_mm` is the dot pitch of the print as measured, in millimeters
/// ([`BoardSpec::nominal_pitch_mm`] for an unmeasured standard print).
/// `board_distance_m` assumes it is exact and that the color intrinsics in
/// `geometry` are right; the board must not move between the frames.
pub fn board_scale(
    geometry: &CameraGeometry,
    frames: &[Frames],
    pitch_mm: f64,
) -> Result<BoardScale, BoardError> {
    if !(pitch_mm.is_finite() && pitch_mm > 0.0) {
        return Err(BoardError::DegeneratePose(format!(
            "pitch {pitch_mm} mm is not a positive length"
        )));
    }
    let mut detections = Vec::new();
    let mut first_error = None;
    for frame in frames {
        match detect_board(&frame.color, &geometry.color).and_then(|d| Ok((d.pose(pitch_mm)?, d))) {
            Ok((pose, detection)) => detections.push((pose, detection)),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    if detections.is_empty() {
        return Err(first_error.unwrap_or(BoardError::NoBoard { blobs: 0 }));
    }

    let distances: Vec<f64> = detections.iter().map(|(p, _)| p.distance_m).collect();
    let board_distance_m = median_of(distances.clone());
    let center = Vector3::new(
        median_of(detections.iter().map(|(p, _)| p.center_m.x).collect()),
        median_of(detections.iter().map(|(p, _)| p.center_m.y).collect()),
        median_of(detections.iter().map(|(p, _)| p.center_m.z).collect()),
    );
    let rotation_sum: Matrix3<f64> = detections.iter().map(|(p, _)| p.rotation).sum();
    let svd = rotation_sum.svd(true, true);
    let mut rotation =
        svd.u.zip(svd.v_t).map(|(u, v)| u * v).ok_or_else(|| {
            BoardError::DegeneratePose("the board's rotations do not average".into())
        })?;
    if rotation.determinant() < 0.0 {
        rotation = -rotation;
    }
    let pose = BoardPose::new(rotation, center, pitch_mm)?;

    let grid_to_px = pose.grid_homography(&geometry.color);
    let points = board_depth_points(geometry, frames, &grid_to_px);
    let no_depth = |inliers| BoardError::NoDepth {
        points: points.len(),
        inliers,
    };
    if points.len() < MIN_DEPTH_POINTS {
        return Err(no_depth(0));
    }
    let params = RansacParams {
        threshold: 0.008 + 0.004 * board_distance_m,
        ..RansacParams::default()
    };
    let fit = ransac_plane(&points, &params).ok_or_else(|| no_depth(0))?;
    if fit.inliers.len() < MIN_DEPTH_INLIERS {
        return Err(no_depth(fit.inliers.len()));
    }

    let plane_in_color = fit.plane.transformed(&geometry.color_to_depth.inverse());
    let direction = center / center.norm();
    let hit = plane_in_color
        .intersect_ray(&Vector3::zeros(), &direction)
        .ok_or_else(|| {
            BoardError::BadFit("the depth plane is parallel to the line of sight".into())
        })?;
    let depth_distance_m = hit.norm();
    let disagreement = plane_in_color
        .normal
        .dot(&pose.normal())
        .abs()
        .clamp(0.0, 1.0)
        .acos()
        .to_degrees();

    let stat = |f: &dyn Fn(&BoardDetection) -> f64| {
        median_of(detections.iter().map(|(_, d)| f(d)).collect())
    };
    let median_diameter = stat(&|d| d.median_dot_diameter_px);
    let detection = DetectionStats {
        dots_matched: stat(&|d| d.dots.len() as f64).round() as usize,
        dots_expected: detections[0].1.dots_expected,
        median_dot_diameter_px: median_diameter,
        footprint_extent_px: [
            stat(&|d| d.footprint_extent_px[0]),
            stat(&|d| d.footprint_extent_px[1]),
        ],
        rms_px: stat(&|d| d.rms_px),
        target_size: TargetSize::classify(median_diameter),
    };
    Ok(BoardScale {
        pitch_mm,
        board_distance_m,
        depth_distance_m,
        ratio: depth_distance_m / board_distance_m,
        frames_used: detections.len(),
        frames_total: frames.len(),
        frame_distances_m: distances,
        depth_points: points.len(),
        depth_inliers: fit.inliers.len(),
        depth_plane_rms_m: fit.rmse,
        plane_disagreement_deg: disagreement,
        view_angle_deg: pose.view_angle_deg,
        detection,
    })
}

#[cfg(test)]
mod tests {
    use image::{GrayImage, Rgb};
    use nalgebra::UnitQuaternion;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::camera::{DepthImage, Extrinsics};
    use crate::plane::{Plane, pixel_ray, project};

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
                Shape::Disk { x, y, r } => (u - x).powi(2) + (v - y).powi(2) <= r * r,
                Shape::Rect { x0, y0, x1, y1 } => u >= x0 && u <= x1 && v >= y0 && v <= y1,
            }
        }

        fn bounds(&self) -> [f64; 4] {
            match *self {
                Shape::Disk { x, y, r } => [x - r, y - r, x + r, y + r],
                Shape::Rect { x0, y0, x1, y1 } => [x0, y0, x1, y1],
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
            for pair in crossings.chunks_exact(2) {
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
                            let g =
                                apply_homography(&to_board, Point2::from(p)).expect("board point");
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
                let ray =
                    pixel_ray(intr, Point2::new(f64::from(x), f64::from(y))).expect("pinhole");
                let mut z = 3.0;
                if let Some(p) = plane.intersect_ray(&Vector3::zeros(), &ray) {
                    let q =
                        scene.rotation.transpose() * (color_from_depth.apply(&p) - scene.center_m);
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
}
