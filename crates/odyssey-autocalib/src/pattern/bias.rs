//! Perspective bias of dot centroids: the center of a dot's elliptical image is not the image
//! of the dot's center.

use nalgebra::{Matrix3, Point2, Vector2, Vector3};

use super::blobs::Blob;
use super::{Match, Pattern, dot_positions, robust_fit};
use crate::plane::apply_homography;

/// Passes of [`remove_center_bias`]. Each pass refits the homography the next
/// one linearizes around; the correction converges geometrically, so three
/// passes reach well below the centroid noise.
const BIAS_PASSES: usize = 3;
/// The correction has converged once no dot's offset moves by more than this.
const BIAS_CONVERGED_PX: f64 = 0.01;
/// Below this the pole is at infinity: the circle does not image to an ellipse, so it has no
/// center to correct.
const MIN_POLE_W: f64 = 1e-12;

/// Offset between the center of a dot's elliptical image and the image of the
/// dot's true center, in pixels, for a pattern-to-pixel homography
/// `unit_to_pixel`. `None` if `dot` is out of range or the projected circle has no finite
/// center.
///
/// A circle seen obliquely images to an ellipse, and perspective foreshortens
/// its far half more than its near half, so the ellipse center (which is where
/// a centroid lands) is not the image of the circle's center. The ellipse
/// center is the pole of the line at infinity with respect to the projected
/// conic. With `M` mapping the unit circle to the image, that pole is
/// `M diag(1, 1, -1) M^T (0, 0, 1)^T`.
///
/// The circle lives in physical pattern coordinates (units of screen height,
/// so the dot is round); the homography takes normalized `(u, v)`, which
/// stretch `x` by the aspect ratio.
///
/// Lens distortion is deliberately not part of this model. The centroids are
/// undistorted point-wise before the correction, which treats a dot as a point
/// for the lens even though its outline is bent slightly; at dot sizes of
/// 1-2% of the screen height that second-order term stays far below the
/// perspective term.
pub fn center_bias(
    pattern: &Pattern,
    dot: usize,
    unit_to_pixel: &Matrix3<f64>,
) -> Option<Vector2<f64>> {
    let d = pattern.dots.get(dot)?;
    let (x, y) = (d.u * pattern.aspect, d.v);
    let physical =
        unit_to_pixel * Matrix3::new(1.0 / pattern.aspect, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0);
    let m = physical * Matrix3::new(d.radius_h, 0.0, x, 0.0, d.radius_h, y, 0.0, 0.0, 1.0);
    let bottom = m.row(2).transpose();
    let pole = m * Matrix3::from_diagonal(&Vector3::new(1.0, 1.0, -1.0)) * bottom;
    if pole.z.abs() < MIN_POLE_W {
        return None;
    }
    let ellipse_center = Point2::new(pole.x / pole.z, pole.y / pole.z);
    let image_of_center = apply_homography(&physical, Point2::new(x, y))?;
    Some(ellipse_center - image_of_center)
}

/// Estimates the image of each dot's true center from its measured centroid by
/// subtracting [`center_bias`] under `unit_to_pixel`. `measured` pairs a dot
/// index with its centroid in undistorted pixels. `None` if the homography
/// leaves any dot degenerate.
pub fn corrected_centers(
    pattern: &Pattern,
    unit_to_pixel: &Matrix3<f64>,
    measured: &[(usize, Point2<f64>)],
) -> Option<Vec<Point2<f64>>> {
    measured
        .iter()
        .map(|&(dot, p)| Some(p - center_bias(pattern, dot, unit_to_pixel)?))
        .collect()
}

/// Homography and matches after bias removal.
pub(super) struct Debiased {
    pub h: Matrix3<f64>,
    /// Matches that survived the refits.
    pub matches: Vec<Match>,
    /// Corrected centers, in the order of `matches`.
    pub centers: Vec<Point2<f64>>,
}

/// Refits the homography on bias-corrected centers until the correction stops
/// changing or [`BIAS_PASSES`] run out. `threshold` is the inlier distance of each refit, in
/// pixels.
pub(super) fn remove_center_bias(
    pattern: &Pattern,
    blobs: &[Blob],
    mut h: Matrix3<f64>,
    mut matches: Vec<Match>,
    threshold: f64,
) -> Debiased {
    let mut centers: Vec<Point2<f64>> = matches.iter().map(|m| blobs[m.blob].undistorted).collect();
    let mut previous = vec![Vector2::zeros(); matches.len()];
    for _ in 0..BIAS_PASSES {
        let Some(bias) = matches
            .iter()
            .map(|m| center_bias(pattern, m.dot, &h))
            .collect::<Option<Vec<_>>>()
        else {
            break;
        };
        let change = bias
            .iter()
            .zip(&previous)
            .map(|(a, b)| (a - b).norm())
            .fold(0.0, f64::max);
        if change < BIAS_CONVERGED_PX {
            break;
        }
        let src = dot_positions(pattern, &matches);
        let dst: Vec<_> = matches
            .iter()
            .zip(&bias)
            .map(|(m, off)| blobs[m.blob].undistorted - off)
            .collect();
        let Some(fit) = robust_fit(&src, &dst, threshold) else {
            break;
        };
        h = fit.h;
        centers = fit.inliers.iter().map(|&i| dst[i]).collect();
        previous = fit.inliers.iter().map(|&i| bias[i]).collect();
        matches = fit.inliers.iter().map(|&i| matches[i]).collect();
    }
    Debiased {
        h,
        matches,
        centers,
    }
}
