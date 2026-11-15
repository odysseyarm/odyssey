//! Online refinement of one unit's near-field camera and the marker pattern it tracks.
//!
//! Each matched frame contributes the image positions of the markers it saw and the ESKF pose
//! at that moment. The solver treats every frame's 6-DoF pose as a free nuisance variable and
//! eliminates it with a Schur complement, leaving a small joint problem over: one focal scale
//! (`fx` and `fy` together), the principal point, and the x/y offsets of every marker except
//! the two that fix the pattern's gauge. Pixel units are whatever the camera model uses (1/64
//! native px on lite1); noise and priors are scaled by the focal length so the constants below
//! do not depend on that unit.
//!
//! shortcut: lens distortion is held at the configured model and observations are the
//! already-undistorted positions. Upgrade if a unit's distortion is large enough that a focal
//! change visibly moves the undistortion.

use std::collections::VecDeque;

use nalgebra::{Isometry3, Matrix2x3, Point3, SMatrix, SVector, Vector2, Vector3};
use odyssey_common::MARKER_PATTERN_LEN;

pub(super) const MARKERS: usize = MARKER_PATTERN_LEN;

/// Markers whose x/y stay at their configured values. Per-frame poses are free, so without two
/// anchors the pattern could slide and turn in its own plane without changing any residual.
const GAUGE: [usize; 2] = [0, 2];
const FREE_MARKERS: usize = MARKERS - GAUGE.len();
const IDX_FOCAL: usize = 0;
const IDX_CX: usize = 1;
const IDX_CY: usize = 2;
const IDX_MARKERS: usize = 3;
const PARAMS: usize = IDX_MARKERS + 2 * FREE_MARKERS;

type Params = SVector<f64, PARAMS>;
type Info = SMatrix<f64, PARAMS, PARAMS>;
type PoseBlock = SMatrix<f64, 6, 6>;

/// Native pixel size relative to the focal length: lite1's 197.6 px focal length is 12645 report
/// units, so one native px is 1/197.6 in normalized image coordinates.
const NORMALIZED_PER_NATIVE_PX: f64 = 1.0 / 197.6;
/// Per-coordinate noise assumed when weighting residuals. The empirical residual inflates it
/// when the data is noisier.
const MEASUREMENT_SIGMA_PX: f64 = 0.1;
const PRIOR_CENTER_SD_PX: f64 = 1.5;
const PRIOR_FOCAL_SD: f64 = 0.05;
const PRIOR_MARKER_SD_M: f64 = 0.01;

const MIN_MARKERS_PER_FRAME: usize = 4;
/// ESKF pose against configured optics: a frame whose own markers disagree by more than this
/// is a mismatch, not calibration error.
const MAX_ADMIT_RESIDUAL_PX: f64 = 6.0;
const MIN_ROTATION_STEP_RAD: f64 = 0.5 * std::f64::consts::PI / 180.0;
const MIN_TRANSLATION_STEP_M: f64 = 0.01;
const MAX_FRAMES: usize = 300;
/// Every fourth admitted frame is held out of the fit and only scores it.
const HELD_OUT_PERIOD: u64 = 4;
const MIN_TRAIN_FRAMES: usize = 50;
const MIN_HELD_OUT_FRAMES: usize = 15;
const SOLVE_INTERVAL_FRAMES: usize = 100;

const MAX_ITERATIONS: usize = 40;
const MAX_DAMPING_TRIES: usize = 10;
const POSE_ITERATIONS: usize = 12;
const MIN_DEPTH_M: f64 = 1e-3;
/// Relative cost drop below which the fit counts as converged.
const CONVERGED_RELATIVE_DROP: f64 = 1e-9;

/// Accept only a focal scale pinned down this well. Fronto-parallel data cannot do it:
/// focal length and distance trade off exactly.
const MAX_FOCAL_SD: f64 = 0.005;
/// Accept only if the held-out reprojection RMS falls to at most this share of what the
/// configured values give.
const MAX_HELD_OUT_RMS_RATIO: f64 = 0.9;
const MAX_FOCAL_SHIFT: f64 = 0.1;
const MAX_CENTER_SHIFT_PX: f64 = 8.0;
const MAX_MARKER_SHIFT_M: f64 = 0.03;

/// Camera parameters in report units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Intrinsics {
    pub(super) fx: f64,
    pub(super) fy: f64,
    pub(super) cx: f64,
    pub(super) cy: f64,
}

/// Camera and pattern the estimator judged better than the configured ones.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Refinement {
    pub(super) intrinsics: Intrinsics,
    pub(super) markers: [Point3<f64>; MARKERS],
    pub(super) focal_sd: f64,
    pub(super) rms_before: f64,
    pub(super) rms_after: f64,
}

#[derive(Debug)]
struct Frame {
    camera_from_world: Isometry3<f64>,
    observations: [Option<Vector2<f64>>; MARKERS],
    held_out: bool,
}

/// Collects matched near-field frames for one camera and one marker pattern.
#[derive(Debug)]
pub(super) struct CalibrationEstimator {
    base: Intrinsics,
    markers: [Point3<f64>; MARKERS],
    /// Scale of each parameter: the solver works in units of its prior sd, which makes the
    /// prior the identity and keeps the normal equations well conditioned.
    scale: Params,
    sigma: f64,
    frames: VecDeque<Frame>,
    last_admitted: Option<Isometry3<f64>>,
    admitted: u64,
    since_solve: usize,
}

struct Model {
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
    markers: [Point3<f64>; MARKERS],
}

/// One frame's contribution to the normal equations, unweighted.
struct Normal {
    h_pp: PoseBlock,
    h_pt: SMatrix<f64, 6, PARAMS>,
    g_p: SVector<f64, 6>,
    h_tt: Info,
    g_t: Params,
    ssr: f64,
}

struct Fit {
    covariance: Info,
    theta: Params,
    ssr: f64,
    residuals: usize,
}

fn free_slot(marker: usize) -> Option<usize> {
    if GAUGE.contains(&marker) {
        return None;
    }
    Some(marker - GAUGE.iter().filter(|anchor| **anchor < marker).count())
}

fn step_pose(pose: &Isometry3<f64>, step: &SVector<f64, 6>) -> Isometry3<f64> {
    let rotation = Vector3::new(step[0], step[1], step[2]);
    let translation = Vector3::new(step[3], step[4], step[5]);
    Isometry3::new(translation, rotation) * pose
}

fn damped_inverse(block: &PoseBlock, lambda: f64) -> Option<PoseBlock> {
    let mut damped = *block;
    for i in 0..6 {
        damped[(i, i)] += lambda * block[(i, i)] + 1e-12;
    }
    Some(damped.cholesky()?.inverse())
}

impl Normal {
    fn zero() -> Self {
        Self {
            h_pp: PoseBlock::zeros(),
            h_pt: SMatrix::zeros(),
            g_p: SVector::zeros(),
            h_tt: Info::zeros(),
            g_t: Params::zeros(),
            ssr: 0.0,
        }
    }
}

impl CalibrationEstimator {
    pub(super) fn new(base: Intrinsics, markers: [Point3<f64>; MARKERS]) -> Self {
        let focal = base.fx;
        let mut scale = Params::zeros();
        scale[IDX_FOCAL] = PRIOR_FOCAL_SD;
        scale[IDX_CX] = PRIOR_CENTER_SD_PX * NORMALIZED_PER_NATIVE_PX * focal;
        scale[IDX_CY] = scale[IDX_CX];
        for i in IDX_MARKERS..PARAMS {
            scale[i] = PRIOR_MARKER_SD_M;
        }
        Self {
            base,
            markers,
            scale,
            sigma: MEASUREMENT_SIGMA_PX * NORMALIZED_PER_NATIVE_PX * focal,
            frames: VecDeque::new(),
            last_admitted: None,
            admitted: 0,
            since_solve: 0,
        }
    }

    /// Record one frame: the camera's pose in the world frame (the ESKF's) and the observed
    /// position of each marker it matched. Returns whether the frame was kept. Frames with too
    /// few markers, a pose nearly equal to the last kept one, or a pose that does not explain
    /// its own markers are dropped.
    pub(super) fn observe(
        &mut self,
        world_from_camera: &Isometry3<f64>,
        observations: [Option<Vector2<f64>>; MARKERS],
    ) -> bool {
        if observations.iter().flatten().count() < MIN_MARKERS_PER_FRAME {
            return false;
        }
        if let Some(last) = &self.last_admitted {
            let rotation = (last.rotation.inverse() * world_from_camera.rotation).angle();
            let translation =
                (last.translation.vector - world_from_camera.translation.vector).norm();
            if rotation < MIN_ROTATION_STEP_RAD && translation < MIN_TRANSLATION_STEP_M {
                return false;
            }
        }
        let camera_from_world = world_from_camera.inverse();
        let model = self.model(&Params::zeros());
        let limit = MAX_ADMIT_RESIDUAL_PX * NORMALIZED_PER_NATIVE_PX * self.base.fx;
        let explains = observations.iter().enumerate().all(|(k, obs)| {
            obs.is_none_or(|obs| (model.project(&camera_from_world, k) - obs).norm() <= limit)
        });
        if !explains {
            return false;
        }
        self.last_admitted = Some(*world_from_camera);
        self.frames.push_back(Frame {
            camera_from_world,
            observations,
            held_out: self.admitted % HELD_OUT_PERIOD == HELD_OUT_PERIOD - 1,
        });
        if self.frames.len() > MAX_FRAMES {
            self.frames.pop_front();
        }
        self.admitted += 1;
        self.since_solve += 1;
        true
    }

    pub(super) fn ready(&self) -> bool {
        self.since_solve >= SOLVE_INTERVAL_FRAMES
    }

    /// Fit the collected frames and return the result if it is trustworthy: focal scale
    /// determined to [`MAX_FOCAL_SD`], plausible shifts, and a held-out reprojection RMS
    /// clearly below that of the configured values.
    pub(super) fn solve(&mut self) -> Option<Refinement> {
        self.since_solve = 0;
        let train: Vec<&Frame> = self.frames.iter().filter(|f| !f.held_out).collect();
        let held_out: Vec<&Frame> = self.frames.iter().filter(|f| f.held_out).collect();
        if train.len() < MIN_TRAIN_FRAMES || held_out.len() < MIN_HELD_OUT_FRAMES {
            return None;
        }
        let fit = self.fit(&train)?;
        let focal_sd = fit.covariance[(IDX_FOCAL, IDX_FOCAL)].sqrt() * self.scale[IDX_FOCAL];
        if !focal_sd.is_finite() || focal_sd >= MAX_FOCAL_SD {
            return None;
        }

        let refined = self.model(&fit.theta);
        let offset = |k: usize| (refined.markers[k] - self.markers[k]).norm();
        let center_limit = MAX_CENTER_SHIFT_PX * NORMALIZED_PER_NATIVE_PX * self.base.fx;
        if (refined.fx / self.base.fx - 1.0).abs() > MAX_FOCAL_SHIFT
            || (refined.cx - self.base.cx).abs() > center_limit
            || (refined.cy - self.base.cy).abs() > center_limit
            || (0..MARKERS).any(|k| offset(k) > MAX_MARKER_SHIFT_M)
        {
            return None;
        }

        let rms_before = self.held_out_rms(&held_out, &self.model(&Params::zeros()))?;
        let rms_after = self.held_out_rms(&held_out, &refined)?;
        if rms_after > MAX_HELD_OUT_RMS_RATIO * rms_before {
            return None;
        }
        Some(Refinement {
            intrinsics: Intrinsics {
                fx: refined.fx,
                fy: refined.fy,
                cx: refined.cx,
                cy: refined.cy,
            },
            markers: refined.markers,
            focal_sd,
            rms_before,
            rms_after,
        })
    }

    fn model(&self, theta: &Params) -> Model {
        let t = |i: usize| theta[i] * self.scale[i];
        let focal = 1.0 + t(IDX_FOCAL);
        let mut markers = self.markers;
        for (k, marker) in markers.iter_mut().enumerate() {
            if let Some(slot) = free_slot(k) {
                marker.x += t(IDX_MARKERS + 2 * slot);
                marker.y += t(IDX_MARKERS + 2 * slot + 1);
            }
        }
        Model {
            fx: self.base.fx * focal,
            fy: self.base.fy * focal,
            cx: self.base.cx + t(IDX_CX),
            cy: self.base.cy + t(IDX_CY),
            markers,
        }
    }

    fn linearize(
        &self,
        pose: &Isometry3<f64>,
        observations: &[Option<Vector2<f64>>; MARKERS],
        model: &Model,
    ) -> Normal {
        let mut normal = Normal::zero();
        let rotation = pose.rotation.to_rotation_matrix();
        for (k, obs) in observations.iter().enumerate() {
            let Some(obs) = obs else { continue };
            let p = pose.transform_point(&model.markers[k]).coords;
            let inv_z = 1.0 / p.z.max(MIN_DEPTH_M);
            let error = model.project(pose, k) - obs;
            let d_proj = Matrix2x3::new(
                model.fx * inv_z,
                0.0,
                -model.fx * p.x * inv_z * inv_z,
                0.0,
                model.fy * inv_z,
                -model.fy * p.y * inv_z * inv_z,
            );
            let mut jp = SMatrix::<f64, 2, 6>::zeros();
            jp.fixed_view_mut::<2, 3>(0, 0)
                .copy_from(&(d_proj * -p.cross_matrix()));
            jp.fixed_view_mut::<2, 3>(0, 3).copy_from(&d_proj);

            let mut jt = SMatrix::<f64, 2, PARAMS>::zeros();
            jt[(0, IDX_FOCAL)] = self.base.fx * p.x * inv_z * self.scale[IDX_FOCAL];
            jt[(1, IDX_FOCAL)] = self.base.fy * p.y * inv_z * self.scale[IDX_FOCAL];
            jt[(0, IDX_CX)] = self.scale[IDX_CX];
            jt[(1, IDX_CY)] = self.scale[IDX_CY];
            if let Some(slot) = free_slot(k) {
                let columns = rotation.matrix().fixed_columns::<2>(0);
                let d_marker = d_proj * columns * PRIOR_MARKER_SD_M;
                jt.fixed_view_mut::<2, 2>(0, IDX_MARKERS + 2 * slot)
                    .copy_from(&d_marker);
            }

            normal.h_pp += jp.transpose() * jp;
            normal.h_pt += jp.transpose() * jt;
            normal.g_p += jp.transpose() * error;
            normal.h_tt += jt.transpose() * jt;
            normal.g_t += jt.transpose() * error;
            normal.ssr += error.norm_squared();
        }
        normal
    }

    /// Weighted data cost plus the prior, which is the identity in scaled parameters.
    fn cost(&self, frames: &[&Frame], poses: &[Isometry3<f64>], theta: &Params) -> f64 {
        let model = self.model(theta);
        let data: f64 = frames
            .iter()
            .zip(poses)
            .map(|(frame, pose)| model.ssr(pose, frame))
            .sum();
        data / (self.sigma * self.sigma) + theta.norm_squared()
    }

    /// Schur complement of the pose blocks. A frame whose pose block cannot be inverted
    /// contributes nothing and does not move.
    fn schur(normals: &[Normal], lambda: f64) -> (Info, Params, Vec<PoseBlock>) {
        let mut info = Info::zeros();
        let mut gradient = Params::zeros();
        let mut inverses = Vec::with_capacity(normals.len());
        for normal in normals {
            let Some(inverse) = damped_inverse(&normal.h_pp, lambda) else {
                inverses.push(PoseBlock::zeros());
                continue;
            };
            let gain = normal.h_pt.transpose() * inverse;
            info += normal.h_tt - gain * normal.h_pt;
            gradient += normal.g_t - gain * normal.g_p;
            inverses.push(inverse);
        }
        (info, gradient, inverses)
    }

    fn fit(&self, frames: &[&Frame]) -> Option<Fit> {
        let weight = 1.0 / (self.sigma * self.sigma);
        let mut poses: Vec<Isometry3<f64>> = frames.iter().map(|f| f.camera_from_world).collect();
        let mut theta = Params::zeros();
        let mut lambda = 1e-3;
        let mut cost = self.cost(frames, &poses, &theta);
        for _ in 0..MAX_ITERATIONS {
            let model = self.model(&theta);
            let normals: Vec<Normal> = frames
                .iter()
                .zip(&poses)
                .map(|(frame, pose)| self.linearize(pose, &frame.observations, &model))
                .collect();
            let mut step = None;
            for _ in 0..MAX_DAMPING_TRIES {
                let (info, gradient, inverses) = Self::schur(&normals, lambda);
                let mut system = info * weight + Info::identity();
                for i in 0..PARAMS {
                    system[(i, i)] += lambda * system[(i, i)];
                }
                let rhs = -(gradient * weight + theta);
                let Some(cholesky) = system.cholesky() else {
                    lambda *= 10.0;
                    continue;
                };
                let d_theta = cholesky.solve(&rhs);
                let new_poses: Vec<Isometry3<f64>> = normals
                    .iter()
                    .zip(&inverses)
                    .zip(&poses)
                    .map(|((normal, inverse), pose)| {
                        let d_pose = -(inverse * (normal.g_p + normal.h_pt * d_theta));
                        step_pose(pose, &d_pose)
                    })
                    .collect();
                let new_theta = theta + d_theta;
                let new_cost = self.cost(frames, &new_poses, &new_theta);
                if new_cost < cost {
                    step = Some((new_poses, new_theta, new_cost));
                    lambda = (lambda / 3.0).max(1e-9);
                    break;
                }
                lambda *= 5.0;
            }
            let Some((new_poses, new_theta, new_cost)) = step else {
                break;
            };
            let drop = cost - new_cost;
            poses = new_poses;
            theta = new_theta;
            cost = new_cost;
            if drop < CONVERGED_RELATIVE_DROP * cost {
                break;
            }
        }

        let model = self.model(&theta);
        let normals: Vec<Normal> = frames
            .iter()
            .zip(&poses)
            .map(|(frame, pose)| self.linearize(pose, &frame.observations, &model))
            .collect();
        let (info, _, _) = Self::schur(&normals, 0.0);
        let ssr: f64 = normals.iter().map(|n| n.ssr).sum();
        let residuals: usize = frames
            .iter()
            .map(|f| 2 * f.observations.iter().flatten().count())
            .sum();
        let covariance = (info * weight + Info::identity()).cholesky()?.inverse();
        Some(
            Fit {
                covariance,
                theta,
                ssr,
                residuals,
            }
            .inflated(frames.len(), self.sigma),
        )
    }

    /// Reprojection RMS over `frames` with each pose refit to the model, so the score does
    /// not depend on how good the ESKF pose was.
    fn held_out_rms(&self, frames: &[&Frame], model: &Model) -> Option<f64> {
        let mut ssr = 0.0;
        let mut residuals = 0;
        for frame in frames {
            let pose = self.refine_pose(frame, model);
            ssr += model.ssr(&pose, frame);
            residuals += 2 * frame.observations.iter().flatten().count();
        }
        (residuals > 0).then(|| (ssr / residuals as f64).sqrt())
    }

    fn refine_pose(&self, frame: &Frame, model: &Model) -> Isometry3<f64> {
        let mut pose = frame.camera_from_world;
        let mut cost = model.ssr(&pose, frame);
        let mut lambda = 1e-6;
        for _ in 0..POSE_ITERATIONS {
            let normal = self.linearize(&pose, &frame.observations, model);
            let Some(inverse) = damped_inverse(&normal.h_pp, lambda) else {
                break;
            };
            let candidate = step_pose(&pose, &-(inverse * normal.g_p));
            let candidate_cost = model.ssr(&candidate, frame);
            if candidate_cost < cost {
                pose = candidate;
                cost = candidate_cost;
                lambda = (lambda / 3.0).max(1e-9);
            } else {
                lambda *= 10.0;
            }
        }
        pose
    }
}

impl Fit {
    /// Noisier data than the weighting assumed widens the covariance by the observed excess.
    fn inflated(mut self, frames: usize, sigma: f64) -> Self {
        let dof = self.residuals as f64 - 6.0 * frames as f64 - PARAMS as f64;
        if dof >= 1.0 {
            let observed = (self.ssr / dof).sqrt();
            self.covariance *= (observed / sigma).powi(2).max(1.0);
        }
        self
    }
}

impl Model {
    fn project(&self, pose: &Isometry3<f64>, marker: usize) -> Vector2<f64> {
        let p = pose.transform_point(&self.markers[marker]);
        let inv_z = 1.0 / p.z.max(MIN_DEPTH_M);
        Vector2::new(
            self.fx * p.x * inv_z + self.cx,
            self.fy * p.y * inv_z + self.cy,
        )
    }

    /// Sum of squared reprojection errors of the frame's observations from `pose`.
    fn ssr(&self, pose: &Isometry3<f64>, frame: &Frame) -> f64 {
        frame
            .observations
            .iter()
            .enumerate()
            .filter_map(|(k, obs)| Some((self.project(pose, k) - (*obs)?).norm_squared()))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::{Rotation3, Translation3, UnitQuaternion};

    use super::*;

    /// lite1's near-field camera in report units (1/64 native px).
    const NOMINAL: Intrinsics = Intrinsics {
        fx: 12645.0,
        fy: 12645.0,
        cx: 10240.0,
        cy: 7680.0,
    };
    const SENSOR: [f64; 2] = [20480.0, 15360.0];
    const NOISE_UNITS: f64 = 0.05 * 64.0;
    /// A 55 inch screen with the bezel pattern the autocalib wizard stores.
    const SCREEN: [f64; 2] = [1.2, 0.68];
    const PATTERN: [[f64; 2]; MARKERS] = [
        [0.2, 1.0],
        [0.2, 0.0],
        [0.8, 0.0],
        [0.5, 0.0],
        [0.5, 1.0],
        [0.8, 1.0],
    ];

    struct Rng(u64);

    impl Rng {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        fn symmetric(&mut self, limit: f64) -> f64 {
            (self.uniform() * 2.0 - 1.0) * limit
        }

        fn gaussian(&mut self, sd: f64) -> f64 {
            let (a, b) = (self.uniform().max(1e-12), self.uniform());
            sd * (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()
        }
    }

    fn pattern() -> [Point3<f64>; MARKERS] {
        PATTERN.map(|[u, v]| Point3::new(u * SCREEN[0], v * SCREEN[1], 0.0))
    }

    struct Scene {
        camera: Intrinsics,
        markers: [Point3<f64>; MARKERS],
    }

    impl Scene {
        fn nominal() -> Self {
            Self {
                camera: NOMINAL,
                markers: pattern(),
            }
        }

        fn perturbed() -> Self {
            let mut markers = pattern();
            markers[1] += Vector3::new(0.003, -0.002, 0.0);
            markers[4] += Vector3::new(-0.002, 0.003, 0.0);
            Self {
                camera: Intrinsics {
                    fx: NOMINAL.fx * 1.02,
                    fy: NOMINAL.fy * 1.02,
                    ..NOMINAL
                },
                markers,
            }
        }

        /// A camera pose (world from camera) within `tilt` radians on each axis and the
        /// image positions the true scene gives from it, with noise.
        fn frame(
            &self,
            rng: &mut Rng,
            tilt: f64,
        ) -> (Isometry3<f64>, [Option<Vector2<f64>>; MARKERS]) {
            let rotation = Rotation3::from_euler_angles(
                rng.symmetric(tilt),
                rng.symmetric(tilt),
                rng.symmetric(tilt),
            );
            let center = Vector3::new(0.5 * SCREEN[0], 0.5 * SCREEN[1], 0.0);
            let aim = Vector3::new(rng.symmetric(0.15), rng.symmetric(0.1), 1.5 + rng.uniform());
            let position = center - rotation * aim;
            let world_from_camera = Isometry3::from_parts(
                Translation3::from(position),
                UnitQuaternion::from_rotation_matrix(&rotation),
            );
            let camera_from_world = world_from_camera.inverse();
            let mut observations = [None; MARKERS];
            for (k, marker) in self.markers.iter().enumerate() {
                let p = camera_from_world.transform_point(marker);
                let u = self.camera.fx * p.x / p.z + self.camera.cx + rng.gaussian(NOISE_UNITS);
                let v = self.camera.fy * p.y / p.z + self.camera.cy + rng.gaussian(NOISE_UNITS);
                if (0.0..SENSOR[0]).contains(&u) && (0.0..SENSOR[1]).contains(&v) {
                    observations[k] = Some(Vector2::new(u, v));
                }
            }
            (world_from_camera, observations)
        }
    }

    /// The ESKF's pose is close to the truth but not exact.
    fn eskf_pose(rng: &mut Rng, truth: &Isometry3<f64>) -> Isometry3<f64> {
        let jitter = Isometry3::new(
            Vector3::new(
                rng.gaussian(0.003),
                rng.gaussian(0.003),
                rng.gaussian(0.005),
            ),
            Vector3::new(
                rng.gaussian(0.002),
                rng.gaussian(0.002),
                rng.gaussian(0.002),
            ),
        );
        jitter * truth
    }

    fn estimator_with_frames(scene: &Scene, tilt: f64, frames: usize) -> CalibrationEstimator {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut estimator = CalibrationEstimator::new(NOMINAL, pattern());
        let mut kept = 0;
        while kept < frames {
            let (truth, observations) = scene.frame(&mut rng, tilt);
            let pose = eskf_pose(&mut rng, &truth);
            kept += usize::from(estimator.observe(&pose, observations));
        }
        estimator
    }

    #[test]
    fn recovers_focal_and_marker_offsets_and_is_accepted() {
        let scene = Scene::perturbed();
        let mut estimator = estimator_with_frames(&scene, 15f64.to_radians(), 240);
        let refinement = estimator.solve().expect("data pins the focal length");
        let focal_error = (refinement.intrinsics.fx / scene.camera.fx - 1.0).abs();
        assert!(focal_error < 0.005, "focal off by {focal_error}");
        for (k, (got, truth)) in refinement.markers.iter().zip(&scene.markers).enumerate() {
            assert!((got - truth).norm() < 0.001, "marker {k}: {got} vs {truth}");
        }
        assert!(refinement.focal_sd < MAX_FOCAL_SD);
        assert!(refinement.rms_after < 0.9 * refinement.rms_before);
    }

    #[test]
    fn fronto_parallel_data_is_rejected() {
        let scene = Scene::perturbed();
        let mut estimator = estimator_with_frames(&scene, 0.0, 240);
        assert_eq!(estimator.solve(), None);
    }

    #[test]
    fn noise_only_data_changes_nothing() {
        let scene = Scene::nominal();
        let mut estimator = estimator_with_frames(&scene, 15f64.to_radians(), 240);
        assert_eq!(estimator.solve(), None);
    }

    #[test]
    fn too_few_frames_are_not_solved() {
        let scene = Scene::perturbed();
        let mut estimator = estimator_with_frames(&scene, 15f64.to_radians(), 30);
        assert_eq!(estimator.solve(), None);
    }

    #[test]
    fn frames_with_a_mismatched_marker_are_dropped() {
        let scene = Scene::nominal();
        let mut rng = Rng(7);
        let (pose, mut observations) = loop {
            let (pose, observations) = scene.frame(&mut rng, 0.1);
            if observations.iter().flatten().count() >= MIN_MARKERS_PER_FRAME {
                break (pose, observations);
            }
        };
        let mut estimator = CalibrationEstimator::new(NOMINAL, pattern());
        let first = observations
            .iter()
            .position(Option::is_some)
            .expect("a marker");
        observations[first] = observations[first].map(|o| o + Vector2::new(2000.0, 0.0));
        assert!(!estimator.observe(&pose, observations));
    }

    #[test]
    fn gauge_markers_are_never_free() {
        assert_eq!(free_slot(0), None);
        assert_eq!(free_slot(2), None);
        let slots: Vec<_> = (0..MARKERS).filter_map(free_slot).collect();
        assert_eq!(slots, (0..FREE_MARKERS).collect::<Vec<_>>());
    }
}
