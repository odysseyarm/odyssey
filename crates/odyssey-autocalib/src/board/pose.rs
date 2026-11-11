//! Rigid board pose from the matched dots: homography decomposition,
//! Levenberg-Marquardt refinement and the correction of the perspective bias
//! of the dots' centroids.

use nalgebra::{DMatrix, DVector, Matrix3, Point2, Rotation3, Vector2, Vector3};

use super::BoardError;
use crate::camera::Intrinsics;
use crate::plane::{apply_homography, project_pinhole};

/// A homography column shorter than this has no direction to decompose.
const MIN_HOMOGRAPHY_COLUMN_NORM: f64 = 1e-12;
/// Residual that stands in for a dot the pose cannot project at all, pixels:
/// large enough to dominate any real fit.
const UNPROJECTABLE_RESIDUAL_PX: f64 = 1e6;
/// Rounds of fitting the pose and correcting the centroids for the bias that
/// pose predicts.
const BIAS_CORRECTION_ROUNDS: usize = 3;
/// The imaged circle has no finite center (it is a parabola or hyperbola)
/// when the homogeneous coordinate of its center is under this.
const MIN_POLE_W: f64 = 1e-18;

/// Levenberg-Marquardt iterations at most.
const MAX_ITERATIONS: usize = 30;
/// Damping attempts at one iteration before it counts as stalled.
const MAX_DAMPING_ATTEMPTS: usize = 8;
/// Damping at the start, relative to the diagonal of `J^T J`.
const INITIAL_DAMPING: f64 = 1e-3;
const MIN_DAMPING: f64 = 1e-9;
/// Damping grows by this factor when the damped system cannot be solved.
const DAMPING_GROWTH_SINGULAR: f64 = 10.0;
/// Damping grows by this factor when a step does not lower the cost, and
/// shrinks by it when a step does.
const DAMPING_FACTOR: f64 = 5.0;
/// Finite-difference step for the Jacobian, in radians for the rotation and
/// in pitches for the translation.
const JACOBIAN_STEP: f64 = 1e-6;
/// Floor of a `J^T J` diagonal entry when it scales the damping, so a
/// parameter that does not move the residuals still gets damped.
const MIN_NORMAL_DIAGONAL: f64 = 1e-9;
/// The fit has converged when a step lowers the cost by less than this
/// fraction of it.
const CONVERGED_RELATIVE_DROP: f64 = 1e-12;
/// Floor of the cost in that fraction.
const MIN_COST: f64 = 1e-30;

fn camera_matrix(k: &Intrinsics) -> Matrix3<f64> {
    Matrix3::new(k.fx, 0.0, k.cx, 0.0, k.fy, k.cy, 0.0, 0.0, 1.0)
}

/// `K [r1 r2 t]`: grid units (plane `z = 0` of the board) to pinhole pixels.
pub(super) fn pose_homography(k: &Intrinsics, r: &Matrix3<f64>, t: &Vector3<f64>) -> Matrix3<f64> {
    camera_matrix(k)
        * Matrix3::from_columns(&[r.column(0).into_owned(), r.column(1).into_owned(), *t])
}

/// Rotation and translation (grid units) from a grid-to-pixel homography.
fn decompose_homography(h: &Matrix3<f64>, k: &Intrinsics) -> Option<(Matrix3<f64>, Vector3<f64>)> {
    let m = camera_matrix(k).try_inverse()? * h;
    let (h1, h2, h3) = (
        m.column(0).into_owned(),
        m.column(1).into_owned(),
        m.column(2).into_owned(),
    );
    let (n1, n2) = (h1.norm(), h2.norm());
    if !(n1.is_finite() && n2.is_finite())
        || n1 < MIN_HOMOGRAPHY_COLUMN_NORM
        || n2 < MIN_HOMOGRAPHY_COLUMN_NORM
    {
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

/// Pixel image of the grid point `g` (board plane `z = 0`) under the pose.
fn reproject(
    k: &Intrinsics,
    r: &Matrix3<f64>,
    t: &Vector3<f64>,
    g: Vector2<f64>,
) -> Option<Point2<f64>> {
    project_pinhole(k, &(r * Vector3::new(g.x, g.y, 0.0) + t))
}

/// Distance of each grid point's projection under the pose from its pixel,
/// infinite for a point that does not project.
pub(super) fn reprojection_errors(
    k: &Intrinsics,
    r: &Matrix3<f64>,
    t: &Vector3<f64>,
    grid: &[Vector2<f64>],
    pixels: &[Point2<f64>],
) -> Vec<f64> {
    grid.iter()
        .zip(pixels)
        .map(|(g, c)| reproject(k, r, t, *g).map_or(f64::INFINITY, |p| (p - c).norm()))
        .collect()
}

/// Dots whose grid positions and pixel observations a pose is fitted to.
struct Observations<'a> {
    intrinsics: &'a Intrinsics,
    grid: &'a [Vector2<f64>],
    pixels: &'a [Point2<f64>],
}

impl Observations<'_> {
    /// Projection minus observation, `x` and `y` of every dot in turn.
    fn residual_vector(&self, r: &Matrix3<f64>, t: &Vector3<f64>) -> DVector<f64> {
        let mut out = DVector::zeros(2 * self.grid.len());
        for (i, (g, px)) in self.grid.iter().zip(self.pixels).enumerate() {
            if let Some(p) = reproject(self.intrinsics, r, t, *g) {
                out[2 * i] = p.x - px.x;
                out[2 * i + 1] = p.y - px.y;
            } else {
                out[2 * i] = UNPROJECTABLE_RESIDUAL_PX;
                out[2 * i + 1] = UNPROJECTABLE_RESIDUAL_PX;
            }
        }
        out
    }

    fn cost(&self, r: &Matrix3<f64>, t: &Vector3<f64>) -> f64 {
        self.residual_vector(r, t).norm_squared()
    }

    /// Forward-difference Jacobian of the residuals with respect to the
    /// pose increment of [`apply_increment`], given the residuals `base` at
    /// the pose itself.
    fn jacobian(&self, r: &Matrix3<f64>, t: &Vector3<f64>, base: &DVector<f64>) -> DMatrix<f64> {
        let mut jac = DMatrix::zeros(base.len(), 6);
        for j in 0..6 {
            let mut delta = [0.0; 6];
            delta[j] = JACOBIAN_STEP;
            let (r2, t2) = apply_increment(r, t, &delta);
            let shifted = self.residual_vector(&r2, &t2);
            jac.set_column(j, &((shifted - base) / JACOBIAN_STEP));
        }
        jac
    }
}

/// The pose moved by a rotation vector `delta[..3]` applied on the left and a
/// translation `delta[3..]`.
fn apply_increment(
    r: &Matrix3<f64>,
    t: &Vector3<f64>,
    delta: &[f64; 6],
) -> (Matrix3<f64>, Vector3<f64>) {
    let dr = Rotation3::from_scaled_axis(Vector3::new(delta[0], delta[1], delta[2]));
    (
        dr.into_inner() * r,
        t + Vector3::new(delta[3], delta[4], delta[5]),
    )
}

/// Levenberg-Marquardt refinement of the rigid pose against the dot centers.
/// The pose is rigid and the plane's scale is tied to the pitch, which a
/// homography decomposition does not enforce.
fn refine_pose(
    observations: &Observations<'_>,
    mut r: Matrix3<f64>,
    mut t: Vector3<f64>,
) -> (Matrix3<f64>, Vector3<f64>) {
    let mut cost = observations.cost(&r, &t);
    let mut mu = INITIAL_DAMPING;
    for _ in 0..MAX_ITERATIONS {
        let base = observations.residual_vector(&r, &t);
        let jac = observations.jacobian(&r, &t, &base);
        let jtj = jac.transpose() * &jac;
        let jtr = jac.transpose() * &base;
        let mut improved = false;
        for _ in 0..MAX_DAMPING_ATTEMPTS {
            let mut damped = jtj.clone();
            for d in 0..6 {
                damped[(d, d)] += mu * jtj[(d, d)].max(MIN_NORMAL_DIAGONAL);
            }
            let Some(solution) = damped.lu().solve(&(-&jtr)) else {
                mu *= DAMPING_GROWTH_SINGULAR;
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
            let (r2, t2) = apply_increment(&r, &t, &delta);
            let new_cost = observations.cost(&r2, &t2);
            if new_cost < cost {
                let relative = (cost - new_cost) / cost.max(MIN_COST);
                r = r2;
                t = t2;
                cost = new_cost;
                mu = (mu / DAMPING_FACTOR).max(MIN_DAMPING);
                improved = true;
                if relative < CONVERGED_RELATIVE_DROP {
                    return (r, t);
                }
                break;
            }
            mu *= DAMPING_FACTOR;
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
    if pole.z.abs() < MIN_POLE_W {
        return None;
    }
    let ellipse_center = Vector2::new(pole.x / pole.z, pole.y / pole.z);
    let projected = apply_homography(h, Point2::from(g))?;
    Some(ellipse_center - projected.coords)
}

/// The rigid pose of the board and the dot centers it was fitted to.
#[derive(Debug)]
pub(super) struct PoseFit {
    /// Board frame to camera frame.
    pub(super) rotation: Matrix3<f64>,
    /// Grid center in the camera frame, in pitches.
    pub(super) translation: Vector3<f64>,
    /// The measured dot centers after the correction for the perspective
    /// bias of a circle's centroid, in the order of the input dots.
    pub(super) centers: Vec<Point2<f64>>,
}

/// Fits the rigid board pose to `measured` pixel centers of the dots at grid
/// positions `grid`, starting from the decomposition of the homography
/// `grid_to_px`. Dots are circles of radius `rho` grid units.
///
/// Alternates between fitting the pose and correcting each centroid for the
/// bias that pose predicts.
///
/// # Errors
///
/// [`BoardError::DegeneratePose`] when `grid_to_px` does not decompose into a
/// rotation and translation, or puts the board behind the camera.
pub(super) fn fit_pose(
    intrinsics: &Intrinsics,
    grid_to_px: &Matrix3<f64>,
    grid: &[Vector2<f64>],
    measured: &[Point2<f64>],
    rho: f64,
) -> Result<PoseFit, BoardError> {
    let (mut r, mut t) = decompose_homography(grid_to_px, intrinsics)
        .ok_or_else(|| BoardError::DegeneratePose("the homography does not decompose".into()))?;
    if t.z <= 0.0 {
        return Err(BoardError::DegeneratePose(
            "the board is not in front of the camera".into(),
        ));
    }

    let mut centers = measured.to_vec();
    for _ in 0..BIAS_CORRECTION_ROUNDS {
        let observations = Observations {
            intrinsics,
            grid,
            pixels: &centers,
        };
        (r, t) = refine_pose(&observations, r, t);
        let h = pose_homography(intrinsics, &r, &t);
        for (i, g) in grid.iter().enumerate() {
            let bias = centroid_bias(&h, *g, rho).unwrap_or_else(Vector2::zeros);
            centers[i] = measured[i] - bias;
        }
    }
    let observations = Observations {
        intrinsics,
        grid,
        pixels: &centers,
    };
    (r, t) = refine_pose(&observations, r, t);
    Ok(PoseFit {
        rotation: r,
        translation: t,
        centers,
    })
}
