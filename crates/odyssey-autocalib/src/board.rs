//! Printed scale board: an independent check of the depth sensor's scale.
//!
//! Tests whether a printed board reveals depth-sensor scale error.
//!
//! The board's geometry holds up on synthetic images. Real hardware
//! validation is still needed. Board measurements do not feed calibration
//! results.
//!
//! The board is a `cols x rows` grid of black dots on white paper
//! (`board.json`), with one dot missing to break the grid's 180-degree
//! symmetry. [`detect_board`] finds the dots in a color image and recovers the
//! board's pose relative to the camera. [`board_scale`] compares the board's
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
//! makes every board distance wrong by exactly `x` percent.
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

use std::sync::LazyLock;

use nalgebra::{Matrix3, Point2, Vector2, Vector3};
use serde::Deserialize;

use crate::camera::Intrinsics;

mod blobs;
mod detect;
mod grid;
mod pose;
mod scale;
#[cfg(test)]
mod tests;

/// Since: Odyssey Desktop 0.6.7.
pub use detect::detect_board;
use pose::pose_homography;
/// Since: Odyssey Desktop 0.6.7.
pub use scale::{BoardScale, DetectionStats, board_scale};

/// Fewest matched dots for a well-conditioned fit. The full board has 34.
///
/// Since: Odyssey Desktop 0.6.7.
pub const MIN_DOTS: usize = 12;
/// Below this many matched dots a hypothesis is not even reported as "too few
/// dots": a handful of coincidentally aligned blobs is indistinguishable from
/// clutter, so the frame is "no board".
const MIN_PLAUSIBLE_DOTS: usize = 6;
/// Minimum diameter for accurate dot-center fitting.
///
/// A 1 px edge blur or demosaicing smear is under a tenth of the dot. The grid
/// spans about twelve dot diameters, so a 0.1 px center error is below 0.1% of
/// its extent.
///
/// Since: Odyssey Desktop 0.6.7.
pub const GOOD_DOT_DIAMETER_PX: f64 = 12.0;
/// Dot diameter below which detections are unreliable.
///
/// Between the two limits, the target is marginal.
///
/// Since: Odyssey Desktop 0.6.7.
pub const MIN_DOT_DIAMETER_PX: f64 = 8.0;

/// The view angle (board normal against the line of sight) above which the
/// pose is rejected as degenerate.
const MAX_VIEW_ANGLE_DEG: f64 = 80.0;
/// A board nearer to the camera's optical center than this, in meters, is
/// taken to be at the center itself: no usable pose.
const MIN_BOARD_DISTANCE_M: f64 = 1e-6;

/// Geometry of the printed board, from `board.json`.
///
/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, Clone, Deserialize)]
pub struct BoardSpec {
    /// Printed size of the standard board, width and height in millimeters.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub board_mm: [f64; 2],
    /// Since: Odyssey Desktop 0.6.7.
    pub cols: u32,
    /// Since: Odyssey Desktop 0.6.7.
    pub rows: u32,
    /// Dot pitch of the standard print in millimeters. Nominal only: the
    /// metric functions take the pitch of the actual print as an input.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub pitch_mm: f64,
    /// Since: Odyssey Desktop 0.6.7.
    pub dot_diameter_mm: f64,
    /// `[col, row]` of every absent dot.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub missing: Vec<[u32; 2]>,
    /// Length of the printed scale bar in millimeters.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub scale_bar_mm: f64,
}

static SPEC: LazyLock<BoardSpec> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../board.json"))
        .expect("board.json is embedded at build time and checked by a unit test")
});

impl BoardSpec {
    /// The board described by `board.json`.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn get() -> &'static Self {
        &SPEC
    }

    /// The pitch `board.json` prints, in millimeters. Use it as the default
    /// until the real print has been measured.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn nominal_pitch_mm() -> f64 {
        SPEC.pitch_mm
    }

    /// Dots on the board, absent ones excluded.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn dots_expected(&self) -> usize {
        (self.cols * self.rows) as usize - self.missing.len()
    }

    /// Dot diameter in pitches.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn dot_diameter_pitches(&self) -> f64 {
        self.dot_diameter_mm / self.pitch_mm
    }

    /// Half the printed board's width and height in pitches.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn board_half_extent_pitches(&self) -> Vector2<f64> {
        Vector2::new(self.board_mm[0], self.board_mm[1]) / (2.0 * self.pitch_mm)
    }

    fn is_missing(&self, col: u32, row: u32) -> bool {
        self.missing.iter().any(|m| m[0] == col && m[1] == row)
    }

    /// Position of dot `(col, row)` in grid units: the origin is the center of
    /// the dot grid, so the grid center is `(0, 0)`.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn dot_position(&self, col: u32, row: u32) -> Vector2<f64> {
        Vector2::new(
            f64::from(col) - f64::from(self.cols - 1) / 2.0,
            f64::from(row) - f64::from(self.rows - 1) / 2.0,
        )
    }
}

/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, thiserror::Error)]
pub enum BoardError {
    /// Since: Odyssey Desktop 0.6.7.
    #[error("no board found ({blobs} dot-like blobs, none formed the dot grid)")]
    NoBoard {
        #[doc = "Since: Odyssey Desktop 0.6.7."]
        blobs: usize,
    },
    /// Since: Odyssey Desktop 0.6.7.
    #[error("only {found} board dots matched, at least {needed} are needed")]
    TooFewDots {
        #[doc = "Since: Odyssey Desktop 0.6.7."]
        found: usize,
        #[doc = "Since: Odyssey Desktop 0.6.7."]
        needed: usize,
    },
    /// Since: Odyssey Desktop 0.6.7.
    #[error(
        "the board's placement is ambiguous: another placement of the grid explains the dots equally well ({matched} matched); show the corner with the missing dot"
    )]
    AmbiguousPlacement {
        #[doc = "Since: Odyssey Desktop 0.6.7."]
        matched: usize,
    },
    /// Since: Odyssey Desktop 0.6.7.
    #[error("bad fit: {0}")]
    BadFit(String),
    /// Since: Odyssey Desktop 0.6.7.
    #[error(
        "no usable depth on the board ({points} points in the board region, {inliers} on a plane)"
    )]
    NoDepth {
        #[doc = "Since: Odyssey Desktop 0.6.7."]
        points: usize,
        #[doc = "Since: Odyssey Desktop 0.6.7."]
        inliers: usize,
    },
    /// Since: Odyssey Desktop 0.6.7.
    #[error("degenerate pose: {0}")]
    DegeneratePose(String),
}

/// How big the dots appear, as a rule of thumb for the target size. See
/// [`GOOD_DOT_DIAMETER_PX`] and [`MIN_DOT_DIAMETER_PX`].
///
/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSize {
    /// Since: Odyssey Desktop 0.6.7.
    TooSmall,
    /// Since: Odyssey Desktop 0.6.7.
    Marginal,
    /// Since: Odyssey Desktop 0.6.7.
    Adequate,
}

impl TargetSize {
    /// Since: Odyssey Desktop 0.6.7.
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

/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchedDot {
    /// Since: Odyssey Desktop 0.6.7.
    pub col: u32,
    /// Since: Odyssey Desktop 0.6.7.
    pub row: u32,
    /// Estimated image of the dot's center in undistorted (pinhole) pixels,
    /// corrected for the perspective bias of the centroid.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub center_px: Point2<f64>,
    /// Equivalent diameter of the dot's image in pixels.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub diameter_px: f64,
    /// Distance of `center_px` from the fitted pose's projection, pixels.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub residual_px: f64,
}

/// Result of finding the board in one color image.
///
/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, Clone)]
pub struct BoardDetection {
    /// Maps grid units (origin at the grid center, `x` right, `y` down, one
    /// unit per pitch) to undistorted pixels. Built from the fitted pose.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub homography: Matrix3<f64>,
    /// Since: Odyssey Desktop 0.6.7.
    pub dots: Vec<MatchedDot>,
    /// Since: Odyssey Desktop 0.6.7.
    pub dots_expected: usize,
    /// Blobs that passed the dot tests, before grid matching.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub blobs_considered: usize,
    /// RMS reprojection error of the matched dots under the fitted rigid pose,
    /// in pixels.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub rms_px: f64,
    /// Median equivalent diameter of the matched dots' images, pixels.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub median_dot_diameter_px: f64,
    /// The printed board's corners (top-left, top-right, bottom-right,
    /// bottom-left) in undistorted pixels.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub footprint_px: [Point2<f64>; 4],
    /// Mean width and height of the printed board's image in pixels.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub footprint_extent_px: [f64; 2],
    /// Rotation from the board frame to the camera frame.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub rotation: Matrix3<f64>,
    /// Position of the grid center in the camera frame, in pitches.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub translation_pitches: Vector3<f64>,
}

impl BoardDetection {
    /// Whether the dots look big enough to trust, by [`TargetSize`].
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn target_size(&self) -> TargetSize {
        TargetSize::classify(self.median_dot_diameter_px)
    }

    /// Metric pose for a print with the given dot pitch in millimeters.
    ///
    /// # Errors
    ///
    /// Returns an error when `pitch_mm` is non-finite or non-positive.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn pose(&self, pitch_mm: f64) -> Result<BoardPose, BoardError> {
        check_pitch_mm(pitch_mm)?;
        let center_m = self.translation_pitches * (pitch_mm / 1000.0);
        BoardPose::new(self.rotation, center_m, pitch_mm)
    }
}

/// Pose of the board in the color camera's frame.
///
/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoardPose {
    /// Board frame to camera frame. The board frame has `x` right, `y` down
    /// and `z` pointing into the board, away from the viewer.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub rotation: Matrix3<f64>,
    /// The center point of the dot grid in the camera frame, meters.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub center_m: Vector3<f64>,
    /// Distance from the camera's optical center to the center point of the
    /// dot grid, meters: `center_m.norm()`.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub distance_m: f64,
    /// Angle between the board normal (pointing at the camera) and the line
    /// of sight to the grid center, degrees.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub view_angle_deg: f64,
    /// Since: Odyssey Desktop 0.6.7.
    pub pitch_mm: f64,
}

impl BoardPose {
    fn new(
        rotation: Matrix3<f64>,
        center_m: Vector3<f64>,
        pitch_mm: f64,
    ) -> Result<Self, BoardError> {
        let distance_m = center_m.norm();
        if !distance_m.is_finite() || distance_m <= MIN_BOARD_DISTANCE_M || center_m.z <= 0.0 {
            return Err(BoardError::DegeneratePose(
                "the board is not in front of the camera".into(),
            ));
        }
        let normal_to_camera = board_normal(&rotation);
        let cos = normal_to_camera
            .dot(&(-center_m / distance_m))
            .clamp(-1.0, 1.0);
        let view_angle_deg = cos.acos().to_degrees();
        if !cos.is_finite() || cos <= 0.0 {
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
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn normal(&self) -> Vector3<f64> {
        board_normal(&self.rotation)
    }

    /// Homography from grid units to undistorted pixels.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub fn grid_homography(&self, intrinsics: &Intrinsics) -> Matrix3<f64> {
        pose_homography(
            intrinsics,
            &self.rotation,
            &(self.center_m / (self.pitch_mm / 1000.0)),
        )
    }
}

fn board_normal(rotation: &Matrix3<f64>) -> Vector3<f64> {
    -(rotation * Vector3::z())
}

fn check_pitch_mm(pitch_mm: f64) -> Result<(), BoardError> {
    if pitch_mm.is_finite() && pitch_mm > 0.0 {
        return Ok(());
    }
    Err(BoardError::DegeneratePose(format!(
        "pitch {pitch_mm} mm is not a positive length"
    )))
}

/// Median of `values`, NaN when empty. Sorts them.
#[expect(
    clippy::manual_midpoint,
    reason = "image-coordinate mean preserves existing rounding"
)]
fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}
