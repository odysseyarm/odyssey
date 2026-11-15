//! Finding the board in one color image: blobs, grid placement, RANSAC
//! inlier selection, rigid pose, quality checks.

use image::RgbImage;
use nalgebra::{Matrix3, Point2, Vector2};

use super::blobs::{DotBlob, find_dots};
use super::grid::{Hypothesis, Lattice, match_grid, matched_points};
use super::pose::{fit_pose, pose_homography, reprojection_errors};
use super::{
    BoardDetection, BoardError, BoardPose, BoardSpec, MIN_DOTS, MIN_PLAUSIBLE_DOTS, MatchedDot,
    median,
};
use crate::camera::Intrinsics;
use crate::plane::{apply_homography, ransac_homography};

/// The RANSAC homography keeps matches within this fraction of the grid's
/// image pitch...
const INLIER_PITCH_FRACTION: f64 = 0.05;
/// ...but at least this many pixels.
const MIN_INLIER_THRESHOLD_PX: f64 = 1.0;
/// RANSAC samples for the inlier homography.
const RANSAC_ITERATIONS: usize = 300;
/// Fixed seed, so a frame always gives the same detection.
const RANSAC_SEED: u64 = 0xB0A2D;
/// Fewest distinct columns, and rows, the inliers must cover: dots along a
/// line do not fix the board's pose.
const MIN_DISTINCT_COLUMNS_AND_ROWS: usize = 3;
/// The fitted pose's RMS reprojection error may be at most this fraction of
/// the median dot diameter...
const MAX_RMS_DOT_FRACTION: f64 = 0.05;
/// ...but never under this many pixels.
const MIN_RMS_LIMIT_PX: f64 = 1.0;

/// Finds the board in a color image.
///
/// The dots are the dark, round, contrasty blobs. The local white level is a
/// morphological closing of the image, so uneven lighting, glare and dark
/// surroundings do not change which blobs count as dots. Printed text and the
/// scale bar fail the shape tests or fail to fit the grid. Blob centers are
/// intensity-weighted centroids undistorted with `intrinsics`, corrected for
/// the perspective bias of a circle's centroid once the pose is known. The
/// grid is matched without any knowledge of the print's size, so the result
/// is in grid units. Ask the detection for a metric pose with
/// [`BoardDetection::pose`].
///
/// # Errors
///
/// [`BoardError::NoBoard`] when no placement of the grid explains at
/// least six blobs, [`BoardError::TooFewDots`] below [`MIN_DOTS`] dots,
/// [`BoardError::AmbiguousPlacement`] when a partial view fits two placements
/// equally well, [`BoardError::BadFit`] when the matched dots do not span the
/// board or do not fit a rigid board, and [`BoardError::DegeneratePose`].
///
/// # Since
/// Odyssey Desktop 0.6.7.
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
    let best = match_grid(&blobs, &lattice, image)?;
    let (kept, grid_to_px) = robust_matches(&best, &blobs, &lattice)?;

    let grid: Vec<Vector2<f64>> = kept.iter().map(|&(_, c)| lattice.cells[c].pos).collect();
    let measured: Vec<Point2<f64>> = kept.iter().map(|&(b, _)| blobs[b].center).collect();
    let rho = spec.dot_diameter_pitches() / 2.0;
    let fit = fit_pose(intrinsics, &grid_to_px, &grid, &measured, rho)?;
    let (r, t) = (fit.rotation, fit.translation);

    let residuals = reprojection_errors(intrinsics, &r, &t, &grid, &fit.centers);
    let rms_px = (residuals.iter().map(|e| e * e).sum::<f64>() / residuals.len() as f64).sqrt();
    let mut diameters: Vec<f64> = kept.iter().map(|&(b, _)| blobs[b].diameter_px).collect();
    let median_dot_diameter_px = median(&mut diameters);
    if !rms_px.is_finite()
        || rms_px > (MAX_RMS_DOT_FRACTION * median_dot_diameter_px).max(MIN_RMS_LIMIT_PX)
    {
        return Err(BoardError::BadFit(format!(
            "reprojection error {rms_px:.2} px against dots {median_dot_diameter_px:.1} px across; the dots do not lie on a rigid flat board"
        )));
    }
    // Run for its checks only (in front, facing the camera, not too oblique).
    // They do not depend on the pitch.
    BoardPose::new(r, t, 1.0)?;

    let homography = pose_homography(intrinsics, &r, &t);
    let (footprint_px, footprint_extent_px) = board_footprint(&homography, spec)?;
    let dots = kept
        .iter()
        .enumerate()
        .map(|(i, &(b, c))| MatchedDot {
            col: lattice.cells[c].col,
            row: lattice.cells[c].row,
            center_px: fit.centers[i],
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

/// Matches as `(blob index, lattice cell index)` pairs, and the homography
/// they were kept under.
type KeptMatches = (Vec<(usize, usize)>, Matrix3<f64>);

/// The matches of `best` that agree with the rest under a RANSAC homography,
/// and that homography. Fails when too few remain or when they do not span
/// at least three columns and three rows of the grid.
fn robust_matches(
    best: &Hypothesis,
    blobs: &[DotBlob],
    lattice: &Lattice,
) -> Result<KeptMatches, BoardError> {
    let (src, dst) = matched_points(&best.matches, blobs, lattice);
    let pitch_px = best
        .center_pitch_px()
        .ok_or_else(|| BoardError::BadFit("the grid homography is degenerate".into()))?;
    let fit = ransac_homography(
        &src,
        &dst,
        (INLIER_PITCH_FRACTION * pitch_px).max(MIN_INLIER_THRESHOLD_PX),
        RANSAC_ITERATIONS,
        RANSAC_SEED,
    )
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
    if columns.len() < MIN_DISTINCT_COLUMNS_AND_ROWS || rows.len() < MIN_DISTINCT_COLUMNS_AND_ROWS {
        return Err(BoardError::BadFit(
            "the matched dots lie along a line and do not fix the board's pose".into(),
        ));
    }
    Ok((kept, fit.h))
}

/// The printed board's corners (top-left, top-right, bottom-right,
/// bottom-left) in undistorted pixels under `homography`, and the mean width
/// and height of its image.
fn board_footprint(
    homography: &Matrix3<f64>,
    spec: &BoardSpec,
) -> Result<([Point2<f64>; 4], [f64; 2]), BoardError> {
    let half = spec.board_half_extent_pitches();
    let corner = |sx: f64, sy: f64| {
        apply_homography(homography, Point2::new(sx * half.x, sy * half.y)).ok_or_else(|| {
            BoardError::DegeneratePose("the board's outline does not project".into())
        })
    };
    let corners = [
        corner(-1.0, -1.0)?,
        corner(1.0, -1.0)?,
        corner(1.0, 1.0)?,
        corner(-1.0, 1.0)?,
    ];
    #[expect(clippy::manual_midpoint, reason = "preserve existing mean rounding")]
    let extent = [
        ((corners[1] - corners[0]).norm() + (corners[2] - corners[3]).norm()) / 2.0,
        ((corners[3] - corners[0]).norm() + (corners[2] - corners[1]).norm()) / 2.0,
    ];
    Ok((corners, extent))
}
