//! Geometry primitives: lens models, planes and homographies.
//!
//! Everything here is plain `f64` math with no knowledge of cameras or
//! patterns beyond [`Intrinsics`], so the unit tests can pin each piece against
//! analytic answers.

use nalgebra::{DMatrix, Matrix3, Point2, SymmetricEigen, Vector2, Vector3};
use rand::rngs::StdRng;
use rand::seq::index::sample;
use rand::SeedableRng;

use crate::camera::{Extrinsics, Intrinsics};

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

/// Projects a camera-frame point to a pixel, including lens distortion.
/// `None` for points at or behind the image plane.
pub fn project(intr: &Intrinsics, p: &Vector3<f64>) -> Option<Point2<f64>> {
    if p.z <= 1e-9 {
        return None;
    }
    let n = Vector2::new(p.x / p.z, p.y / p.z);
    Some(normalized_to_pixel(intr, distort_normalized(intr, n)))
}

/// Projects a camera-frame point with the pinhole model only. This is the
/// pixel grid that [`undistort_pixel`] produces.
pub fn project_pinhole(intr: &Intrinsics, p: &Vector3<f64>) -> Option<Point2<f64>> {
    if p.z <= 1e-9 {
        return None;
    }
    Some(Point2::new(p.x / p.z * intr.fx + intr.cx, p.y / p.z * intr.fy + intr.cy))
}

const UNDISTORT_ITERATIONS: usize = 40;
const UNDISTORT_CONVERGED_PX: f64 = 0.01;
const UNDISTORT_ACCEPT_PX: f64 = 0.5;

/// Inverts the lens model: returns normalized, undistorted coordinates `(x, y)`
/// of the ray through `pixel`. The model has no closed-form inverse, so this
/// runs a fixed-point iteration and keeps the best iterate. Returns `None` when
/// the iteration does not reach half a pixel, which happens only far outside
/// the calibrated image area.
pub fn undistort(intr: &Intrinsics, pixel: Point2<f64>) -> Option<Vector2<f64>> {
    let [k1, k2, k3, k4, k5, k6] = intr.k;
    let [p1, p2] = intr.p;
    let xd = (pixel.x - intr.cx) / intr.fx;
    let yd = (pixel.y - intr.cy) / intr.fy;

    let (mut x, mut y) = (xd, yd);
    let mut best = (Vector2::new(xd, yd), f64::INFINITY);
    for _ in 0..UNDISTORT_ITERATIONS {
        let r2 = x * x + y * y;
        let r4 = r2 * r2;
        let r6 = r4 * r2;
        let inverse_radial = (1.0 + k4 * r2 + k5 * r4 + k6 * r6) / (1.0 + k1 * r2 + k2 * r4 + k3 * r6);
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
pub fn undistort_pixel(intr: &Intrinsics, pixel: Point2<f64>) -> Option<Point2<f64>> {
    undistort(intr, pixel).map(|n| Point2::new(n.x * intr.fx + intr.cx, n.y * intr.fy + intr.cy))
}

/// The inverse of [`undistort_pixel`]: where the lens puts a pinhole pixel.
pub fn distort_pixel(intr: &Intrinsics, pixel: Point2<f64>) -> Point2<f64> {
    let n = Vector2::new((pixel.x - intr.cx) / intr.fx, (pixel.y - intr.cy) / intr.fy);
    normalized_to_pixel(intr, distort_normalized(intr, n))
}

/// Direction `(x, y, 1)` of the ray through a distorted pixel, in the camera
/// frame. Scaling it by a depth gives the point at that z-depth.
pub fn pixel_ray(intr: &Intrinsics, pixel: Point2<f64>) -> Option<Vector3<f64>> {
    undistort(intr, pixel).map(|n| Vector3::new(n.x, n.y, 1.0))
}

/// A plane through `point` with unit `normal`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub point: Vector3<f64>,
    pub normal: Vector3<f64>,
}

impl Plane {
    pub fn new(point: Vector3<f64>, normal: Vector3<f64>) -> Self {
        Self { point, normal: normal.normalize() }
    }

    /// Orthogonal distance, positive on the side the normal points to.
    pub fn signed_distance(&self, p: &Vector3<f64>) -> f64 {
        self.normal.dot(&(p - self.point))
    }

    /// Where the ray `origin + t * direction` meets the plane, for `t > 0`.
    /// Closed form: the depth along a ray is linear in `t`, so no search is
    /// needed.
    pub fn intersect_ray(&self, origin: &Vector3<f64>, direction: &Vector3<f64>) -> Option<Vector3<f64>> {
        let denom = self.normal.dot(direction);
        if denom.abs() < 1e-9 {
            return None;
        }
        let t = self.normal.dot(&(self.point - origin)) / denom;
        (t > 0.0).then(|| origin + direction * t)
    }

    /// The same plane expressed in another frame.
    pub fn transformed(&self, to: &Extrinsics) -> Plane {
        Plane { point: to.apply(&self.point), normal: to.rotation * self.normal }
    }
}

/// Least-squares plane through `points`: the centroid and the direction of
/// least variance, which is the right singular vector of the centered points
/// for the smallest singular value. `None` for fewer than three points or a
/// degenerate (collinear) cloud.
pub fn fit_plane(points: &[Vector3<f64>]) -> Option<Plane> {
    if points.len() < 3 {
        return None;
    }
    let centroid = points.iter().sum::<Vector3<f64>>() / points.len() as f64;
    let mut cov = Matrix3::zeros();
    for p in points {
        let d = p - centroid;
        cov += d * d.transpose();
    }
    let eig = SymmetricEigen::new(cov);
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| eig.eigenvalues[a].total_cmp(&eig.eigenvalues[b]));
    let (smallest, middle) = (eig.eigenvalues[order[0]], eig.eigenvalues[order[1]]);
    if middle.is_nan() || middle <= 1e-12 || !smallest.is_finite() {
        return None;
    }
    let normal = eig.eigenvectors.column(order[0]).into_owned();
    Some(Plane::new(centroid, normal))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RansacParams {
    /// Points per hypothesis. Larger samples average depth noise inside each
    /// hypothesis, which suits the thousands of mostly-correct points a screen
    /// yields.
    pub sample_size: usize,
    /// Orthogonal distance in meters below which a point supports a plane.
    pub threshold: f64,
    pub max_trials: usize,
    /// Probability of having drawn one outlier-free sample before stopping.
    pub confidence: f64,
    /// Hypotheses are scored on at most this many points; the winner is then
    /// refit on all of them.
    pub score_points: usize,
    pub seed: u64,
}

impl Default for RansacParams {
    fn default() -> Self {
        Self { sample_size: 30, threshold: 0.01, max_trials: 5000, confidence: 0.99, score_points: 4000, seed: 0x0D155EA }
    }
}

#[derive(Debug, Clone)]
pub struct PlaneFit {
    /// Normal oriented away from the camera at the origin.
    pub plane: Plane,
    /// Root-mean-square orthogonal distance of the inliers, meters.
    pub rmse: f64,
    pub max_error: f64,
    /// Indices into the input of the points within the threshold of the final
    /// plane.
    pub inliers: Vec<usize>,
}

fn inlier_stats(plane: &Plane, points: &[Vector3<f64>], threshold: f64) -> (Vec<usize>, f64, f64) {
    let mut inliers = Vec::new();
    let (mut sum_sq, mut max) = (0.0, 0.0f64);
    for (i, p) in points.iter().enumerate() {
        let e = plane.signed_distance(p).abs();
        if e <= threshold {
            inliers.push(i);
            sum_sq += e * e;
            max = max.max(e);
        }
    }
    let rmse = if inliers.is_empty() { 0.0 } else { (sum_sq / inliers.len() as f64).sqrt() };
    (inliers, rmse, max)
}

/// Robust plane fit.
///
/// Inlier tests use the orthogonal distance, not the residual along the
/// camera's z axis: the latter shrinks with the cosine of the screen's tilt
/// and is undefined for a plane seen edge-on, while the orthogonal distance
/// keeps the threshold meaning "meters from the surface" for every pose. The
/// seeded generator makes the result reproducible.
pub fn ransac_plane(points: &[Vector3<f64>], params: &RansacParams) -> Option<PlaneFit> {
    let n = points.len();
    if n < params.sample_size.max(3) {
        return None;
    }
    let mut rng = StdRng::seed_from_u64(params.seed);

    let score_set: Vec<Vector3<f64>> = if n > params.score_points {
        sample(&mut rng, n, params.score_points).into_iter().map(|i| points[i]).collect()
    } else {
        points.to_vec()
    };

    let mut best: Option<(Plane, usize)> = None;
    let mut trials_needed = params.max_trials;
    let mut trial = 0;
    let mut sample_points = Vec::with_capacity(params.sample_size);
    while trial < trials_needed.min(params.max_trials) {
        trial += 1;
        sample_points.clear();
        sample_points.extend(sample(&mut rng, n, params.sample_size).into_iter().map(|i| points[i]));
        let Some(plane) = fit_plane(&sample_points) else { continue };
        let support = score_set.iter().filter(|p| plane.signed_distance(p).abs() <= params.threshold).count();
        if best.as_ref().is_none_or(|(_, s)| support > *s) {
            best = Some((plane, support));
            let w = support as f64 / score_set.len() as f64;
            let all_good = w.powi(params.sample_size as i32);
            trials_needed = if all_good >= 1.0 - 1e-12 {
                trial
            } else if all_good <= 0.0 {
                params.max_trials
            } else {
                ((1.0 - params.confidence).ln() / (-all_good).ln_1p()).ceil().max(1.0) as usize
            };
        }
    }

    let (mut plane, _) = best?;
    // Two refit rounds let the inlier set settle on the refined plane instead
    // of the noisy sample hypothesis.
    for _ in 0..2 {
        let (inliers, _, _) = inlier_stats(&plane, points, params.threshold);
        let inlier_points: Vec<_> = inliers.iter().map(|&i| points[i]).collect();
        plane = fit_plane(&inlier_points)?;
    }
    let (inliers, rmse, max_error) = inlier_stats(&plane, points, params.threshold);
    if inliers.len() < 3 {
        return None;
    }

    if plane.normal.dot(&plane.point) < 0.0 {
        plane.normal = -plane.normal;
    }
    Some(PlaneFit { plane, rmse, max_error, inliers })
}

/// Applies a 3x3 homography to a point.
pub fn apply_homography(h: &Matrix3<f64>, p: Point2<f64>) -> Option<Point2<f64>> {
    let v = h * Vector3::new(p.x, p.y, 1.0);
    (v.z.abs() > 1e-12).then(|| Point2::new(v.x / v.z, v.y / v.z))
}

/// Hartley normalization: translates the centroid to the origin and scales the
/// mean distance to `sqrt(2)`, which keeps the DLT well conditioned.
fn normalization(points: &[Point2<f64>]) -> Option<Matrix3<f64>> {
    let c = points.iter().map(|p| p.coords).sum::<Vector2<f64>>() / points.len() as f64;
    let mean_dist = points.iter().map(|p| (p.coords - c).norm()).sum::<f64>() / points.len() as f64;
    if mean_dist.is_nan() || mean_dist <= 1e-12 {
        return None;
    }
    let s = std::f64::consts::SQRT_2 / mean_dist;
    Some(Matrix3::new(s, 0.0, -s * c.x, 0.0, s, -s * c.y, 0.0, 0.0, 1.0))
}

/// Normalized direct linear transform: the homography `H` with `dst ~ H src`
/// that minimizes the algebraic error over all pairs. Needs at least four pairs
/// with no three collinear.
pub fn fit_homography(src: &[Point2<f64>], dst: &[Point2<f64>]) -> Option<Matrix3<f64>> {
    assert_eq!(src.len(), dst.len());
    let n = src.len();
    if n < 4 {
        return None;
    }
    let ts = normalization(src)?;
    let td = normalization(dst)?;

    // The system has 2n rows but nalgebra's thin SVD only yields as many right
    // singular vectors as rows, so pad to nine rows to keep the null vector.
    let rows = (2 * n).max(9);
    let mut a = DMatrix::<f64>::zeros(rows, 9);
    for (i, (s, d)) in src.iter().zip(dst).enumerate() {
        let s = ts * Vector3::new(s.x, s.y, 1.0);
        let d = td * Vector3::new(d.x, d.y, 1.0);
        let r0 = [-s.x, -s.y, -1.0, 0.0, 0.0, 0.0, d.x * s.x, d.x * s.y, d.x];
        let r1 = [0.0, 0.0, 0.0, -s.x, -s.y, -1.0, d.y * s.x, d.y * s.y, d.y];
        for j in 0..9 {
            a[(2 * i, j)] = r0[j];
            a[(2 * i + 1, j)] = r1[j];
        }
    }
    let svd = a.svd(false, true);
    let v_t = svd.v_t?;
    let (min_idx, _) = svd.singular_values.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1))?;
    let h = v_t.row(min_idx);
    let hn = Matrix3::new(h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8]);
    let h = td.try_inverse()? * hn * ts;
    if !h.iter().all(|v| v.is_finite()) {
        return None;
    }
    let scale = if h[(2, 2)].abs() > 1e-12 { h[(2, 2)] } else { h.norm() };
    Some(h / scale)
}

#[derive(Debug, Clone)]
pub struct HomographyFit {
    pub h: Matrix3<f64>,
    pub inliers: Vec<usize>,
    /// Root-mean-square transfer error of the inliers in `dst` units.
    pub rmse: f64,
}

fn transfer_errors(h: &Matrix3<f64>, src: &[Point2<f64>], dst: &[Point2<f64>]) -> Vec<f64> {
    src.iter()
        .zip(dst)
        .map(|(s, d)| apply_homography(h, *s).map_or(f64::INFINITY, |p| (p - d).norm()))
        .collect()
}

fn non_degenerate(points: &[Point2<f64>; 4]) -> bool {
    for skip in 0..4 {
        let t: Vec<_> = (0..4).filter(|&i| i != skip).map(|i| points[i]).collect();
        let area2 = (t[1] - t[0]).perp(&(t[2] - t[0])).abs();
        let scale = (t[1] - t[0]).norm() * (t[2] - t[0]).norm();
        if area2 < 1e-3 * scale.max(1e-12) {
            return false;
        }
    }
    true
}

/// RANSAC around [`fit_homography`]: hypotheses come from four random pairs, a
/// pair supports one when its transfer error in `dst` is at most `threshold`,
/// and the winner is refit on its inliers.
pub fn ransac_homography(
    src: &[Point2<f64>],
    dst: &[Point2<f64>],
    threshold: f64,
    max_trials: usize,
    seed: u64,
) -> Option<HomographyFit> {
    let n = src.len();
    if n < 4 {
        return None;
    }
    let mut rng = StdRng::seed_from_u64(seed);
    let mut best: Option<(usize, f64)> = None;
    let mut best_inliers: Vec<usize> = Vec::new();
    let mut trials_needed = max_trials;
    let mut trial = 0;
    while trial < trials_needed.min(max_trials) {
        trial += 1;
        let idx = if n == 4 { vec![0, 1, 2, 3] } else { sample(&mut rng, n, 4).into_vec() };
        let s = [src[idx[0]], src[idx[1]], src[idx[2]], src[idx[3]]];
        let d = [dst[idx[0]], dst[idx[1]], dst[idx[2]], dst[idx[3]]];
        if !non_degenerate(&s) || !non_degenerate(&d) {
            if n == 4 {
                return None;
            }
            continue;
        }
        let Some(h) = fit_homography(&s, &d) else { continue };
        let errors = transfer_errors(&h, src, dst);
        let (count, sum) = errors.iter().filter(|e| **e <= threshold).fold((0usize, 0.0), |(c, s), e| (c + 1, s + e));
        if best.is_none_or(|(bc, bs)| count > bc || (count == bc && sum < bs)) {
            best = Some((count, sum));
            best_inliers = errors.iter().enumerate().filter(|(_, e)| **e <= threshold).map(|(i, _)| i).collect();
            let w = count as f64 / n as f64;
            let all_good = w.powi(4);
            trials_needed = if all_good >= 1.0 - 1e-12 {
                trial
            } else if all_good <= 0.0 {
                max_trials
            } else {
                ((1.0 - 0.999f64).ln() / (-all_good).ln_1p()).ceil().max(1.0) as usize
            };
        }
        if n == 4 {
            break;
        }
    }

    let refit = |ids: &[usize]| {
        let s: Vec<_> = ids.iter().map(|&i| src[i]).collect();
        let d: Vec<_> = ids.iter().map(|&i| dst[i]).collect();
        fit_homography(&s, &d)
    };
    let mut inliers = best_inliers;
    for _ in 0..3 {
        if inliers.len() < 4 {
            return None;
        }
        let h = refit(&inliers)?;
        let errors = transfer_errors(&h, src, dst);
        inliers = errors.iter().enumerate().filter(|(_, e)| **e <= threshold).map(|(i, _)| i).collect();
    }
    if inliers.len() < 4 {
        return None;
    }
    let h = refit(&inliers)?;
    let errors = transfer_errors(&h, src, dst);
    let sq: f64 = inliers.iter().map(|&i| errors[i] * errors[i]).sum();
    Some(HomographyFit { h, rmse: (sq / inliers.len() as f64).sqrt(), inliers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    fn intrinsics() -> Intrinsics {
        Intrinsics {
            width: 1280,
            height: 720,
            fx: 900.0,
            fy: 905.0,
            cx: 641.0,
            cy: 358.0,
            k: [-0.08, 0.02, -0.003, 0.0, 0.0, 0.0],
            p: [0.0008, -0.0005],
        }
    }

    #[test]
    fn distortion_round_trips_across_the_image() {
        let intr = intrinsics();
        for &(u, v) in &[(0.0, 0.0), (1279.0, 719.0), (640.0, 360.0), (10.5, 700.2), (1200.0, 30.0)] {
            let pixel = Point2::new(u, v);
            let ray = pixel_ray(&intr, pixel).unwrap();
            let back = project(&intr, &(ray * 2.5)).unwrap();
            assert!((back - pixel).norm() < 0.02, "{pixel:?} -> {back:?}");
        }
    }

    #[test]
    fn undistort_pixel_matches_pinhole_projection() {
        let intr = intrinsics();
        let p = Vector3::new(0.4, -0.2, 2.0);
        let distorted = project(&intr, &p).unwrap();
        let pinhole = project_pinhole(&intr, &p).unwrap();
        let undistorted = undistort_pixel(&intr, distorted).unwrap();
        assert!((undistorted - pinhole).norm() < 0.02);
    }

    #[test]
    fn plane_fit_recovers_known_plane_with_outliers() {
        let truth = Plane::new(Vector3::new(0.1, -0.05, 2.0), Vector3::new(0.2, -0.1, -1.0));
        let mut rng = StdRng::seed_from_u64(7);
        let u = truth.normal.cross(&Vector3::x()).normalize();
        let v = truth.normal.cross(&u);
        let mut points = Vec::new();
        for _ in 0..4000 {
            let (a, b): (f64, f64) = (rng.random_range(-0.6..0.6), rng.random_range(-0.4..0.4));
            let noise: f64 = rng.random_range(-0.002..0.002);
            points.push(truth.point + u * a + v * b + truth.normal * noise);
        }
        for _ in 0..200 {
            points.push(Vector3::new(rng.random_range(-1.0..1.0), rng.random_range(-1.0..1.0), rng.random_range(1.0..3.0)));
        }
        let fit = ransac_plane(&points, &RansacParams::default()).unwrap();
        assert!(fit.plane.normal.dot(&fit.plane.point) > 0.0, "normal points away from the camera");
        assert!(fit.plane.normal.dot(&truth.normal).abs() > 1.0 - 1e-5);
        assert!(fit.plane.signed_distance(&truth.point).abs() < 5e-4);
        assert!(fit.rmse < 0.0015, "rmse {}", fit.rmse);
        assert!(fit.inliers.len() >= 4000 - 20);
    }

    #[test]
    fn ray_plane_intersection_is_exact() {
        let plane = Plane::new(Vector3::new(0.0, 0.0, 3.0), Vector3::new(0.0, 0.3, 1.0));
        let dir = Vector3::new(0.12, -0.2, 1.0);
        let hit = plane.intersect_ray(&Vector3::zeros(), &dir).unwrap();
        assert!(plane.signed_distance(&hit).abs() < 1e-12);
        assert!((hit.xy() / hit.z - dir.xy()).norm() < 1e-12);
        assert!(plane.intersect_ray(&Vector3::zeros(), &Vector3::new(1.0, 0.0, 0.0)).is_none());
        assert!(plane.intersect_ray(&Vector3::zeros(), &Vector3::new(0.0, 0.0, -1.0)).is_none());
    }

    fn truth_homography() -> Matrix3<f64> {
        Matrix3::new(120.0, 14.0, 300.0, -9.0, 95.0, 120.0, 0.0004, 0.0002, 1.0)
    }

    #[test]
    fn dlt_recovers_exact_homography() {
        let h = truth_homography();
        let src: Vec<_> = (0..6).flat_map(|i| (0..4).map(move |j| Point2::new(i as f64 * 0.2, j as f64 * 0.3))).collect();
        let dst: Vec<_> = src.iter().map(|p| apply_homography(&h, *p).unwrap()).collect();
        let fit = fit_homography(&src, &dst).unwrap();
        for p in &src {
            let a = apply_homography(&h, *p).unwrap();
            let b = apply_homography(&fit, *p).unwrap();
            assert!((a - b).norm() < 1e-8);
        }
    }

    #[test]
    fn dlt_with_four_points_matches_truth() {
        let h = truth_homography();
        let src = [Point2::new(0.0, 0.0), Point2::new(1.0, 0.0), Point2::new(1.0, 1.0), Point2::new(0.0, 1.0)];
        let dst = src.map(|p| apply_homography(&h, p).unwrap());
        let fit = fit_homography(&src, &dst).unwrap();
        let probe = Point2::new(0.37, 0.61);
        let a = apply_homography(&h, probe).unwrap();
        let b = apply_homography(&fit, probe).unwrap();
        assert!((a - b).norm() < 1e-8);
    }

    #[test]
    fn ransac_homography_rejects_gross_outliers() {
        let h = truth_homography();
        let mut rng = StdRng::seed_from_u64(3);
        let mut src = Vec::new();
        let mut dst = Vec::new();
        for i in 0..15 {
            for j in 0..9 {
                let p = Point2::new(i as f64 / 14.0, j as f64 / 8.0);
                let mut q = apply_homography(&h, p).unwrap();
                q += Vector2::new(rng.random_range(-0.1..0.1), rng.random_range(-0.1..0.1));
                src.push(p);
                dst.push(q);
            }
        }
        let bad = [3usize, 17, 40, 41, 77, 100];
        for &i in &bad {
            dst[i] += Vector2::new(35.0, -22.0);
        }
        let fit = ransac_homography(&src, &dst, 1.0, 500, 11).unwrap();
        for &i in &bad {
            assert!(!fit.inliers.contains(&i), "outlier {i} accepted");
        }
        assert_eq!(fit.inliers.len(), src.len() - bad.len());
        let probe = Point2::new(0.5, 0.5);
        let err = (apply_homography(&fit.h, probe).unwrap() - apply_homography(&h, probe).unwrap()).norm();
        assert!(err < 0.2, "err {err}");
        assert!(fit.rmse < 0.15);
    }
}
