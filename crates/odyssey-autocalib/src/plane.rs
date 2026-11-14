//! Geometry primitives: lens models, planes and homographies.
//!
//! Everything here is plain `f64` math with no knowledge of cameras or
//! patterns beyond [`Intrinsics`], so the unit tests can pin each piece against
//! analytic answers.

use nalgebra::{DMatrix, Matrix3, Point2, SymmetricEigen, Vector2, Vector3};
use rand::SeedableRng as _;
use rand::rngs::StdRng;
use rand::seq::index::sample;

use crate::camera::{Extrinsics, Intrinsics};

/// Camera-frame depth (meters) at or below which a point counts as behind the
/// image plane and has no pixel.
const MIN_PROJECTION_DEPTH_M: f64 = 1e-9;

/// A ray whose direction has a dot product with the plane normal below this
/// is parallel to the plane and never meets it.
const PARALLEL_EPSILON: f64 = 1e-9;

/// A point set whose spread (a variance or a mean distance, in the points'
/// own units) is below this is a single point or a line, which fixes no plane
/// or homography.
const DEGENERATE_EPSILON: f64 = 1e-12;

/// A homogeneous coordinate or overall homography scale below this magnitude
/// counts as zero: the point is at infinity.
pub(crate) const HOMOGENEOUS_EPSILON: f64 = 1e-12;

/// Applies the Brown-Conrady model to normalized image coordinates.
fn distort_normalized(intr: &Intrinsics, n: Vector2<f64>) -> Vector2<f64> {
    let [k1, k2, k3, k4, k5, k6] = intr.k;
    let [p1, p2] = intr.p;
    let (x, y) = (n.x, n.y);
    let r2 = x * x + y * y;
    let r4 = r2 * r2;
    let r6 = r4 * r2;
    let radial = (1.0 + k1 * r2 + k2 * r4 + k3 * r6) / (1.0 + k4 * r2 + k5 * r4 + k6 * r6);
    Vector2::new(
        x * radial + 2.0 * p1 * x * y + p2 * (r2 + 2.0 * x * x),
        y * radial + p1 * (r2 + 2.0 * y * y) + 2.0 * p2 * x * y,
    )
}

fn normalized_to_pixel(intr: &Intrinsics, n: Vector2<f64>) -> Point2<f64> {
    Point2::new(n.x * intr.fx + intr.cx, n.y * intr.fy + intr.cy)
}

fn pixel_to_normalized(intr: &Intrinsics, pixel: Point2<f64>) -> Vector2<f64> {
    Vector2::new((pixel.x - intr.cx) / intr.fx, (pixel.y - intr.cy) / intr.fy)
}

/// Normalized image coordinates `(x / z, y / z)` of a camera-frame point, or
/// `None` at or behind the image plane.
fn normalized_coordinates(p: &Vector3<f64>) -> Option<Vector2<f64>> {
    if !p.iter().all(|v| v.is_finite()) || p.z <= MIN_PROJECTION_DEPTH_M {
        return None;
    }
    Some(Vector2::new(p.x / p.z, p.y / p.z))
}

/// Projects a camera-frame point to a pixel, including lens distortion.
/// `None` for points at or behind the image plane.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn project(intr: &Intrinsics, p: &Vector3<f64>) -> Option<Point2<f64>> {
    let n = normalized_coordinates(p)?;
    let pixel = normalized_to_pixel(intr, distort_normalized(intr, n));
    (pixel.x.is_finite() && pixel.y.is_finite()).then_some(pixel)
}

/// Projects a camera-frame point with the pinhole model only. This is the
/// pixel grid that [`undistort_pixel`] produces.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn project_pinhole(intr: &Intrinsics, p: &Vector3<f64>) -> Option<Point2<f64>> {
    let n = normalized_coordinates(p)?;
    let pixel = normalized_to_pixel(intr, n);
    (pixel.x.is_finite() && pixel.y.is_finite()).then_some(pixel)
}

const UNDISTORT_ITERATIONS: usize = 40;
const UNDISTORT_CONVERGED_PX: f64 = 0.01;
const UNDISTORT_ACCEPT_PX: f64 = 0.5;

/// Returns normalized, undistorted coordinates `(x, y)` for ray through `pixel`.
///
/// The lens model has no closed-form inverse, so fixed-point iteration keeps
/// the best iterate. Returns `None` if error remains above half a pixel, which
/// only happens far outside the calibrated image area.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn undistort(intr: &Intrinsics, pixel: Point2<f64>) -> Option<Vector2<f64>> {
    let [k1, k2, k3, k4, k5, k6] = intr.k;
    let [p1, p2] = intr.p;
    let distorted = pixel_to_normalized(intr, pixel);
    let (xd, yd) = (distorted.x, distorted.y);

    let (mut x, mut y) = (xd, yd);
    let mut best = (distorted, f64::INFINITY);
    for _ in 0..UNDISTORT_ITERATIONS {
        let r2 = x * x + y * y;
        let r4 = r2 * r2;
        let r6 = r4 * r2;
        let inverse_radial =
            (1.0 + k4 * r2 + k5 * r4 + k6 * r6) / (1.0 + k1 * r2 + k2 * r4 + k3 * r6);
        let dx = p1 * 2.0 * x * y + p2 * (r2 + 2.0 * x * x);
        let dy = p2 * 2.0 * x * y + p1 * (r2 + 2.0 * y * y);
        x = (xd - dx) * inverse_radial;
        y = (yd - dy) * inverse_radial;
        if !x.is_finite() || !y.is_finite() {
            break;
        }

        let reprojected = normalized_to_pixel(intr, distort_normalized(intr, Vector2::new(x, y)));
        let err = (reprojected - pixel).norm();
        if err < best.1 {
            best = (Vector2::new(x, y), err);
        }
        if err < UNDISTORT_CONVERGED_PX {
            break;
        }
    }
    (best.1 <= UNDISTORT_ACCEPT_PX).then_some(best.0)
}

/// The pinhole pixel that sees the same ray as the distorted `pixel`.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn undistort_pixel(intr: &Intrinsics, pixel: Point2<f64>) -> Option<Point2<f64>> {
    undistort(intr, pixel).map(|n| normalized_to_pixel(intr, n))
}

/// The inverse of [`undistort_pixel`]: where the lens puts a pinhole pixel.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn distort_pixel(intr: &Intrinsics, pixel: Point2<f64>) -> Point2<f64> {
    let n = pixel_to_normalized(intr, pixel);
    normalized_to_pixel(intr, distort_normalized(intr, n))
}

/// Direction `(x, y, 1)` of the ray through a distorted pixel, in the camera
/// frame. Scaling it by a depth gives the point at that z-depth.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn pixel_ray(intr: &Intrinsics, pixel: Point2<f64>) -> Option<Vector3<f64>> {
    undistort(intr, pixel).map(|n| Vector3::new(n.x, n.y, 1.0))
}

/// Direction `(x, y, 1)` of the ray through `pixel` under the pinhole model,
/// which is how undistorted pixels map to rays.
pub(crate) fn pinhole_ray(intr: &Intrinsics, pixel: Point2<f64>) -> Vector3<f64> {
    let n = pixel_to_normalized(intr, pixel);
    Vector3::new(n.x, n.y, 1.0)
}

/// A plane through `point` with unit `normal`.
///
/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    /// Since: Odyssey Desktop 0.6.5.
    pub point: Vector3<f64>,
    /// Since: Odyssey Desktop 0.6.5.
    pub normal: Vector3<f64>,
}

impl Plane {
    /// Since: Odyssey Desktop 0.6.5.
    pub fn new(point: Vector3<f64>, normal: Vector3<f64>) -> Self {
        Self {
            point,
            normal: normal.normalize(),
        }
    }

    /// Orthogonal distance, positive on the side the normal points to.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub fn signed_distance(&self, p: &Vector3<f64>) -> f64 {
        self.normal.dot(&(p - self.point))
    }

    /// The `t > 0` at which `origin + t * direction` meets the plane. Closed
    /// form: the depth along a ray is linear in `t`, so no search is needed.
    pub(crate) fn ray_parameter(
        &self,
        origin: &Vector3<f64>,
        direction: &Vector3<f64>,
    ) -> Option<f64> {
        let denom = self.normal.dot(direction);
        if denom.abs() < PARALLEL_EPSILON {
            return None;
        }
        let t = self.normal.dot(&(self.point - origin)) / denom;
        (t.is_finite() && t > 0.0).then_some(t)
    }

    /// Where the ray `origin + t * direction` meets the plane, for `t > 0`.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub fn intersect_ray(
        &self,
        origin: &Vector3<f64>,
        direction: &Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        let t = self.ray_parameter(origin, direction)?;
        let hit = origin + direction * t;
        hit.iter().all(|v| v.is_finite()).then_some(hit)
    }

    /// The same plane expressed in another frame.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    #[must_use]
    pub fn transformed(&self, to: &Extrinsics) -> Self {
        Self {
            point: to.apply(&self.point),
            normal: to.rotation * self.normal,
        }
    }
}

/// Fits a least-squares plane through `points`.
///
/// Uses the centroid and smallest-eigenvalue vector of the centered scatter
/// matrix. Returns `None` for fewer than three points, non-finite points, or a
/// collinear cloud.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn fit_plane(points: &[Vector3<f64>]) -> Option<Plane> {
    if points.len() < 3 || points.iter().any(|p| !p.iter().all(|v| v.is_finite())) {
        return None;
    }
    let centroid = points.iter().sum::<Vector3<f64>>() / points.len() as f64;
    let mut cov = Matrix3::zeros();
    for p in points {
        let d = p - centroid;
        cov += d * d.transpose();
    }
    if !cov.iter().all(|v| v.is_finite()) {
        return None;
    }
    let eig = SymmetricEigen::new(cov);
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| eig.eigenvalues[a].total_cmp(&eig.eigenvalues[b]));
    let (smallest, middle) = (eig.eigenvalues[order[0]], eig.eigenvalues[order[1]]);
    if !middle.is_finite() || middle <= DEGENERATE_EPSILON || !smallest.is_finite() {
        return None;
    }
    let normal = eig.eigenvectors.column(order[0]).into_owned();
    Some(Plane::new(centroid, normal))
}

/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RansacParams {
    /// Points per hypothesis. Larger samples average depth noise inside each
    /// hypothesis, which suits the thousands of mostly-correct points a screen
    /// yields.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub sample_size: usize,
    /// Orthogonal distance in meters below which a point supports a plane.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub threshold: f64,
    /// Since: Odyssey Desktop 0.6.5.
    pub max_trials: usize,
    /// Probability of having drawn one outlier-free sample before stopping.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub confidence: f64,
    /// Hypotheses are scored on at most this many points. The winner is then
    /// refit on all of them.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub score_points: usize,
    /// Since: Odyssey Desktop 0.6.5.
    pub seed: u64,
}

impl Default for RansacParams {
    fn default() -> Self {
        Self {
            sample_size: 30,
            threshold: 0.01,
            max_trials: 5000,
            confidence: 0.99,
            score_points: 4000,
            seed: 0x00D1_55EA,
        }
    }
}

/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone)]
pub struct PlaneFit {
    /// Normal oriented away from the camera at the origin.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub plane: Plane,
    /// Root-mean-square orthogonal distance of the inliers, meters.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub rmse: f64,
    /// Since: Odyssey Desktop 0.6.5.
    pub max_error: f64,
    /// Indices into the input of the points within the threshold of the final
    /// plane.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub inliers: Vec<usize>,
}

/// The points within a threshold of a plane, and how closely they hug it.
struct InlierStats {
    indices: Vec<usize>,
    rmse: f64,
    max_error: f64,
}

fn inlier_stats(plane: &Plane, points: &[Vector3<f64>], threshold: f64) -> InlierStats {
    let mut indices = Vec::new();
    let (mut sum_sq, mut max_error) = (0.0, 0.0f64);
    for (i, p) in points.iter().enumerate() {
        let e = plane.signed_distance(p).abs();
        if e <= threshold {
            indices.push(i);
            sum_sq += e * e;
            max_error = max_error.max(e);
        }
    }
    let rmse = if indices.is_empty() {
        0.0
    } else {
        (sum_sq / indices.len() as f64).sqrt()
    };
    InlierStats {
        indices,
        rmse,
        max_error,
    }
}

/// Tolerance below which an inlier share of `1` counts as "every sample is
/// outlier-free", ending the trial search at once.
const ALL_INLIERS_TOLERANCE: f64 = 1e-12;

/// How many RANSAC trials are needed to have drawn at least one outlier-free
/// sample of `sample_size` with probability `confidence`, when a share
/// `inlier_share` of the data supports the best hypothesis so far.
/// `trial` is the number of trials done, `max_trials` the cap.
fn trials_for_confidence(
    inlier_share: f64,
    sample_size: usize,
    confidence: f64,
    trial: usize,
    max_trials: usize,
) -> usize {
    let all_good = inlier_share.powi(sample_size as i32);
    if all_good >= 1.0 - ALL_INLIERS_TOLERANCE {
        trial
    } else if all_good <= 0.0 {
        max_trials
    } else {
        ((1.0 - confidence).ln() / (-all_good).ln_1p())
            .ceil()
            .max(1.0) as usize
    }
}

/// Plane hypotheses from random samples of `points`, scored by how many of
/// `score_set` they support. Returns the best one, which is still noisy.
fn best_plane_hypothesis(
    points: &[Vector3<f64>],
    score_set: &[Vector3<f64>],
    params: &RansacParams,
    rng: &mut StdRng,
) -> Option<Plane> {
    let mut best: Option<(Plane, usize)> = None;
    let mut trials_needed = params.max_trials;
    let mut trial = 0;
    let mut sample_points = Vec::with_capacity(params.sample_size);
    while trial < trials_needed.min(params.max_trials) {
        trial += 1;
        sample_points.clear();
        sample_points.extend(
            sample(rng, points.len(), params.sample_size)
                .into_iter()
                .map(|i| points[i]),
        );
        let Some(plane) = fit_plane(&sample_points) else {
            continue;
        };
        let support = score_set
            .iter()
            .filter(|p| plane.signed_distance(p).abs() <= params.threshold)
            .count();
        if best.as_ref().is_none_or(|(_, s)| support > *s) {
            best = Some((plane, support));
            trials_needed = trials_for_confidence(
                support as f64 / score_set.len() as f64,
                params.sample_size,
                params.confidence,
                trial,
                params.max_trials,
            );
        }
    }
    best.map(|(plane, _)| plane)
}

/// Refit rounds of the winning plane on its own inliers. Two let the inlier
/// set settle on the refined plane instead of the noisy sample hypothesis.
const PLANE_REFIT_ROUNDS: usize = 2;

/// RANSAC plane fit, refit on its inliers. `None` for invalid `params`, a
/// non-finite point, or fewer than three final inliers.
///
/// Inlier tests use the orthogonal distance, not the residual along the
/// camera's z axis: the latter shrinks with the cosine of the screen's tilt
/// and is undefined for a plane seen edge-on, while the orthogonal distance
/// keeps the threshold meaning "meters from the surface" for every pose. The
/// seeded generator makes the result reproducible.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn ransac_plane(points: &[Vector3<f64>], params: &RansacParams) -> Option<PlaneFit> {
    let n = points.len();
    if params.sample_size < 3
        || n < params.sample_size
        || params.score_points < 3
        || !params.threshold.is_finite()
        || params.threshold <= 0.0
        || !params.confidence.is_finite()
        || !(0.0..1.0).contains(&params.confidence)
        || params.confidence == 0.0
        || points.iter().any(|p| !p.iter().all(|v| v.is_finite()))
    {
        return None;
    }
    let mut rng = StdRng::seed_from_u64(params.seed);

    let score_set: Vec<Vector3<f64>> = if n > params.score_points {
        sample(&mut rng, n, params.score_points)
            .into_iter()
            .map(|i| points[i])
            .collect()
    } else {
        points.to_vec()
    };

    let mut plane = best_plane_hypothesis(points, &score_set, params, &mut rng)?;
    for _ in 0..PLANE_REFIT_ROUNDS {
        let inliers = inlier_stats(&plane, points, params.threshold).indices;
        let inlier_points: Vec<_> = inliers.iter().map(|&i| points[i]).collect();
        plane = fit_plane(&inlier_points)?;
    }
    let InlierStats {
        indices: inliers,
        rmse,
        max_error,
    } = inlier_stats(&plane, points, params.threshold);
    if inliers.len() < 3 {
        return None;
    }

    if plane.normal.dot(&plane.point) < 0.0 {
        plane.normal = -plane.normal;
    }
    Some(PlaneFit {
        plane,
        rmse,
        max_error,
        inliers,
    })
}

/// Applies a 3x3 homography to a point. `None` when it maps to infinity or to
/// a non-finite point.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn apply_homography(h: &Matrix3<f64>, p: Point2<f64>) -> Option<Point2<f64>> {
    let v = h * Vector3::new(p.x, p.y, 1.0);
    if !v.iter().all(|coordinate| coordinate.is_finite()) || v.z.abs() <= HOMOGENEOUS_EPSILON {
        return None;
    }
    let point = Point2::new(v.x / v.z, v.y / v.z);
    (point.x.is_finite() && point.y.is_finite()).then_some(point)
}

/// Hartley normalization: translates the centroid to the origin and scales the
/// mean distance to `sqrt(2)`, which keeps the DLT well conditioned.
fn normalization(points: &[Point2<f64>]) -> Option<Matrix3<f64>> {
    let c = points.iter().map(|p| p.coords).sum::<Vector2<f64>>() / points.len() as f64;
    let mean_dist = points.iter().map(|p| (p.coords - c).norm()).sum::<f64>() / points.len() as f64;
    if !mean_dist.is_finite() || mean_dist <= DEGENERATE_EPSILON {
        return None;
    }
    let s = std::f64::consts::SQRT_2 / mean_dist;
    Some(Matrix3::new(
        s,
        0.0,
        -s * c.x,
        0.0,
        s,
        -s * c.y,
        0.0,
        0.0,
        1.0,
    ))
}

/// Fewest point pairs that determine a homography.
const MIN_HOMOGRAPHY_PAIRS: usize = 4;

/// The direct-linear-transform system `A h = 0` for the normalized pairs, two
/// rows per pair.
///
/// It has 2n rows but nalgebra's thin SVD only yields as many right singular
/// vectors as rows, so it is padded with zero rows to nine to keep the null
/// vector.
fn dlt_system(
    src: &[Point2<f64>],
    dst: &[Point2<f64>],
    src_normalization: &Matrix3<f64>,
    dst_normalization: &Matrix3<f64>,
) -> DMatrix<f64> {
    let rows = (2 * src.len()).max(9);
    let mut a = DMatrix::<f64>::zeros(rows, 9);
    for (i, (s, d)) in src.iter().zip(dst).enumerate() {
        let s = src_normalization * Vector3::new(s.x, s.y, 1.0);
        let d = dst_normalization * Vector3::new(d.x, d.y, 1.0);
        let r0 = [-s.x, -s.y, -1.0, 0.0, 0.0, 0.0, d.x * s.x, d.x * s.y, d.x];
        let r1 = [0.0, 0.0, 0.0, -s.x, -s.y, -1.0, d.y * s.x, d.y * s.y, d.y];
        for (j, (&first, &second)) in r0.iter().zip(&r1).enumerate() {
            a[(2 * i, j)] = first;
            a[(2 * i + 1, j)] = second;
        }
    }
    a
}

/// Normalized direct linear transform: the homography `H` with `dst ~ H src`
/// that minimizes the algebraic error over all pairs. Needs at least four pairs
/// with no three collinear.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn fit_homography(src: &[Point2<f64>], dst: &[Point2<f64>]) -> Option<Matrix3<f64>> {
    if src.len() != dst.len()
        || src.len() < MIN_HOMOGRAPHY_PAIRS
        || src
            .iter()
            .chain(dst)
            .any(|p| !p.x.is_finite() || !p.y.is_finite())
    {
        return None;
    }
    let ts = normalization(src)?;
    let td = normalization(dst)?;

    let svd = dlt_system(src, dst, &ts, &td).svd(false, true);
    // A homography needs eight independent constraints. Collinear or repeated
    // pairs otherwise yield an arbitrary null-space vector, not a mapping.
    let largest = svd.singular_values.iter().copied().fold(0.0, f64::max);
    if svd
        .singular_values
        .iter()
        .filter(|&&s| s > largest * DEGENERATE_EPSILON)
        .count()
        < 8
    {
        return None;
    }
    let v_t = svd.v_t?;
    let (min_idx, _) = svd
        .singular_values
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))?;
    let h = v_t.row(min_idx);
    let hn = Matrix3::new(h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8]);
    // The unit-norm null vector must describe an invertible projective map,
    // not a collapse onto a destination line.
    if hn.determinant().abs() <= DEGENERATE_EPSILON {
        return None;
    }
    let h = td.try_inverse()? * hn * ts;
    if !h.iter().all(|v| v.is_finite()) {
        return None;
    }
    let scale = if h[(2, 2)].abs() > HOMOGENEOUS_EPSILON {
        h[(2, 2)]
    } else {
        h.norm()
    };
    Some(h / scale)
}

/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone)]
pub struct HomographyFit {
    /// Since: Odyssey Desktop 0.6.5.
    pub h: Matrix3<f64>,
    /// Since: Odyssey Desktop 0.6.5.
    pub inliers: Vec<usize>,
    /// Root-mean-square transfer error of the inliers in `dst` units.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub rmse: f64,
}

fn transfer_errors(h: &Matrix3<f64>, src: &[Point2<f64>], dst: &[Point2<f64>]) -> Vec<f64> {
    src.iter()
        .zip(dst)
        .map(|(s, d)| apply_homography(h, *s).map_or(f64::INFINITY, |p| (p - d).norm()))
        .collect()
}

fn indices_within(errors: &[f64], threshold: f64) -> Vec<usize> {
    let mut indices = Vec::new();
    for (i, &error) in errors.iter().enumerate() {
        if error <= threshold {
            indices.push(i);
        }
    }
    indices
}

/// Smallest sine of the angle at the first vertex of any three of four points
/// for them to count as in general position.
const MIN_TRIANGLE_SINE: f64 = 1e-3;

/// Whether no three of the four points are (nearly) collinear.
fn non_degenerate(points: &[Point2<f64>; 4]) -> bool {
    for skip in 0..4 {
        let t: Vec<_> = (0..4).filter(|&i| i != skip).map(|i| points[i]).collect();
        let area2 = (t[1] - t[0]).perp(&(t[2] - t[0])).abs();
        let scale = (t[1] - t[0]).norm() * (t[2] - t[0]).norm();
        if area2 < MIN_TRIANGLE_SINE * scale.max(DEGENERATE_EPSILON) {
            return false;
        }
    }
    true
}

/// Probability, in [`ransac_homography`], of having drawn one outlier-free
/// sample before stopping.
const HOMOGRAPHY_CONFIDENCE: f64 = 0.999;

/// Refits of the winning homography on its own inliers, before the final fit.
const HOMOGRAPHY_REFIT_ROUNDS: usize = 3;

/// Hypotheses from four random pairs. Returns the inliers (by transfer error)
/// of the one with the most support, ties broken by the smaller error sum.
/// `None` when four pairs are all there is and they are degenerate.
fn best_homography_inliers(
    src: &[Point2<f64>],
    dst: &[Point2<f64>],
    threshold: f64,
    max_trials: usize,
    seed: u64,
) -> Option<Vec<usize>> {
    let n = src.len();
    // With exactly four pairs there is one hypothesis and nothing to sample.
    let exact = n == MIN_HOMOGRAPHY_PAIRS;
    let mut rng = StdRng::seed_from_u64(seed);
    let mut best: Option<(usize, f64)> = None;
    let mut best_inliers: Vec<usize> = Vec::new();
    let mut trials_needed = max_trials;
    let mut trial = 0;
    while trial < trials_needed.min(max_trials) {
        trial += 1;
        let idx: [usize; 4] = if exact {
            [0, 1, 2, 3]
        } else {
            let picked = sample(&mut rng, n, MIN_HOMOGRAPHY_PAIRS);
            [
                picked.index(0),
                picked.index(1),
                picked.index(2),
                picked.index(3),
            ]
        };
        let s = idx.map(|i| src[i]);
        let d = idx.map(|i| dst[i]);
        if !non_degenerate(&s) || !non_degenerate(&d) {
            if exact {
                return None;
            }
            continue;
        }
        let Some(h) = fit_homography(&s, &d) else {
            continue;
        };
        let errors = transfer_errors(&h, src, dst);
        let (mut count, mut sum) = (0usize, 0.0);
        for &error in errors.iter().filter(|&&e| e <= threshold) {
            count += 1;
            sum += error;
        }
        if best.is_none_or(|(best_count, best_sum)| {
            count > best_count || (count == best_count && sum < best_sum)
        }) {
            best = Some((count, sum));
            best_inliers = indices_within(&errors, threshold);
            trials_needed = trials_for_confidence(
                count as f64 / n as f64,
                MIN_HOMOGRAPHY_PAIRS,
                HOMOGRAPHY_CONFIDENCE,
                trial,
                max_trials,
            );
        }
        if exact {
            break;
        }
    }
    Some(best_inliers)
}

/// RANSAC around [`fit_homography`]: hypotheses come from four random pairs, a
/// pair supports one when its transfer error in `dst` is at most `threshold`,
/// and the winner is refit on its inliers.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn ransac_homography(
    src: &[Point2<f64>],
    dst: &[Point2<f64>],
    threshold: f64,
    max_trials: usize,
    seed: u64,
) -> Option<HomographyFit> {
    if src.len() != dst.len()
        || src.len() < MIN_HOMOGRAPHY_PAIRS
        || !threshold.is_finite()
        || threshold <= 0.0
        || src
            .iter()
            .chain(dst)
            .any(|p| !p.x.is_finite() || !p.y.is_finite())
    {
        return None;
    }
    let mut inliers = best_homography_inliers(src, dst, threshold, max_trials, seed)?;

    let refit = |ids: &[usize]| {
        let s: Vec<_> = ids.iter().map(|&i| src[i]).collect();
        let d: Vec<_> = ids.iter().map(|&i| dst[i]).collect();
        fit_homography(&s, &d)
    };
    for _ in 0..HOMOGRAPHY_REFIT_ROUNDS {
        if inliers.len() < MIN_HOMOGRAPHY_PAIRS {
            return None;
        }
        let h = refit(&inliers)?;
        inliers = indices_within(&transfer_errors(&h, src, dst), threshold);
    }
    if inliers.len() < MIN_HOMOGRAPHY_PAIRS {
        return None;
    }
    let h = refit(&inliers)?;
    let errors = transfer_errors(&h, src, dst);
    let sq: f64 = inliers.iter().map(|&i| errors[i] * errors[i]).sum();
    Some(HomographyFit {
        h,
        rmse: (sq / inliers.len() as f64).sqrt(),
        inliers,
    })
}

#[cfg(test)]
mod tests;
