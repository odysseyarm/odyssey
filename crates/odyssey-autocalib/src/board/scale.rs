//! Comparing the board's distance from its apparent size with the depth
//! sensor's distance to the same surface.

use nalgebra::{Matrix3, Point2, Vector2, Vector3};

use super::{
    BoardDetection, BoardError, BoardPose, BoardSpec, TargetSize, check_pitch_mm, detect_board,
    median,
};
use crate::camera::{CameraGeometry, Frames};
use crate::plane::{RansacParams, apply_homography, pixel_ray, project_pinhole, ransac_plane};

/// Depth points in the board region needed before a plane is fitted.
const MIN_DEPTH_POINTS: usize = 60;
/// Inliers of the depth plane needed.
const MIN_DEPTH_INLIERS: usize = 50;
/// Depths outside this range (meters) are sensor noise or dropouts, not the
/// board.
const DEPTH_RANGE_M: (f64, f64) = (0.1, 10.0);
/// The depth plane's inlier threshold is this constant (meters) plus
/// [`PLANE_THRESHOLD_PER_METER`] times the board distance, since depth noise
/// grows with distance.
const PLANE_THRESHOLD_BASE_M: f64 = 0.008;
/// See [`PLANE_THRESHOLD_BASE_M`].
const PLANE_THRESHOLD_PER_METER: f64 = 0.004;

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

/// Detects the board in every frame; the frames that find it, with the pose
/// for `pitch_mm`. The first error if none does.
fn detect_in_frames(
    geometry: &CameraGeometry,
    frames: &[Frames],
    pitch_mm: f64,
) -> Result<Vec<(BoardPose, BoardDetection)>, BoardError> {
    let mut found = Vec::new();
    let mut first_error = None;
    for frame in frames {
        match detect_board(&frame.color, &geometry.color).and_then(|d| Ok((d.pose(pitch_mm)?, d))) {
            Ok(pair) => found.push(pair),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    if found.is_empty() {
        return Err(first_error.unwrap_or(BoardError::NoBoard { blobs: 0 }));
    }
    Ok(found)
}

/// The board pose the frames agree on: the median grid center and the
/// rotation closest (by SVD) to the sum of the frames' rotations.
fn consensus_pose(
    detections: &[(BoardPose, BoardDetection)],
    pitch_mm: f64,
) -> Result<BoardPose, BoardError> {
    let center = Vector3::new(
        median(
            &mut detections
                .iter()
                .map(|(p, _)| p.center_m.x)
                .collect::<Vec<_>>(),
        ),
        median(
            &mut detections
                .iter()
                .map(|(p, _)| p.center_m.y)
                .collect::<Vec<_>>(),
        ),
        median(
            &mut detections
                .iter()
                .map(|(p, _)| p.center_m.z)
                .collect::<Vec<_>>(),
        ),
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
    BoardPose::new(rotation, center, pitch_mm)
}

fn detection_stats(detections: &[(BoardPose, BoardDetection)]) -> DetectionStats {
    let stat = |f: &dyn Fn(&BoardDetection) -> f64| {
        median(&mut detections.iter().map(|(_, d)| f(d)).collect::<Vec<_>>())
    };
    let median_diameter = stat(&|d| d.median_dot_diameter_px);
    DetectionStats {
        dots_matched: stat(&|d| d.dots.len() as f64).round() as usize,
        dots_expected: detections[0].1.dots_expected,
        median_dot_diameter_px: median_diameter,
        footprint_extent_px: [
            stat(&|d| d.footprint_extent_px[0]),
            stat(&|d| d.footprint_extent_px[1]),
        ],
        rms_px: stat(&|d| d.rms_px),
        target_size: TargetSize::classify(median_diameter),
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
    // A pixel needs a valid depth in at least half of the frames.
    let needed = frames.len().div_ceil(2);
    let mut samples = Vec::with_capacity(frames.len());
    let mut points = Vec::new();
    for y in 0..h {
        for x in 0..w {
            samples.clear();
            samples.extend(frames.iter().filter_map(|f| f.depth_m(x, y)));
            if samples.len() < needed.max(1) {
                continue;
            }
            let d = median(&mut samples);
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
/// to; the parent `board` module documentation defines it exactly.
///
/// `pitch_mm` is the dot pitch of the print as measured, in millimeters
/// ([`BoardSpec::nominal_pitch_mm`] for an unmeasured standard print).
/// `board_distance_m` assumes it is exact and that the color intrinsics in
/// `geometry` are right; the board must not move between the frames.
///
/// # Errors
///
/// [`BoardError::DegeneratePose`] for a non-finite or non-positive `pitch_mm`
/// or rotations that do not average; the first detection error when no frame
/// finds the board; [`BoardError::NoDepth`] when too few depth points fall on
/// the board or too few fit one plane; [`BoardError::BadFit`] for invalid camera
/// data or when the depth plane is parallel to the line of sight.
pub fn board_scale(
    geometry: &CameraGeometry,
    frames: &[Frames],
    pitch_mm: f64,
) -> Result<BoardScale, BoardError> {
    check_pitch_mm(pitch_mm)?;
    geometry
        .validate()
        .map_err(|error| BoardError::BadFit(error.to_string()))?;
    for frame in frames {
        frame
            .validate(geometry)
            .map_err(|error| BoardError::BadFit(error.to_string()))?;
    }
    let detections = detect_in_frames(geometry, frames, pitch_mm)?;
    let distances: Vec<f64> = detections.iter().map(|(p, _)| p.distance_m).collect();
    let board_distance_m = median(&mut distances.clone());
    let pose = consensus_pose(&detections, pitch_mm)?;

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
        threshold: PLANE_THRESHOLD_BASE_M + PLANE_THRESHOLD_PER_METER * board_distance_m,
        ..RansacParams::default()
    };
    let fit = ransac_plane(&points, &params).ok_or_else(|| no_depth(0))?;
    if fit.inliers.len() < MIN_DEPTH_INLIERS {
        return Err(no_depth(fit.inliers.len()));
    }

    let plane_in_color = fit.plane.transformed(&geometry.color_to_depth.inverse());
    let direction = pose.center_m / pose.center_m.norm();
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
        detection: detection_stats(&detections),
    })
}
