//! Renderer-neutral synthetic scene labels. No tracker state or rendered pixels live here.

use std::collections::HashSet;

use nalgebra::{Quaternion, UnitQuaternion, Vector3};
use odyssey_common::ScreenCalibration;
use serde::{Deserialize, Serialize};

/// Since: Odyssey Desktop 0.7.0.
pub const SCENE_SCHEMA_VERSION: u32 = 1;

/// World and camera axes are +X right, +Y down, +Z forward.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SceneCoordinateBasis {
    /// Since: Odyssey Desktop 0.7.0.
    ScreenXRightYDownZForward,
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDescription {
    /// Since: Odyssey Desktop 0.7.0.
    pub schema_version: u32,
    /// Since: Odyssey Desktop 0.7.0.
    pub provenance: String,
    /// Since: Odyssey Desktop 0.7.0.
    pub coordinate_basis: SceneCoordinateBasis,
    /// Since: Odyssey Desktop 0.7.0.
    pub screens: Vec<SceneScreen>,
    /// Since: Odyssey Desktop 0.7.0.
    pub cameras: Vec<SceneCamera>,
    /// Since: Odyssey Desktop 0.7.0.
    pub frames: Vec<SceneFrame>,
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneScreen {
    /// Since: Odyssey Desktop 0.7.0.
    pub id: u8,
    /// Planar fit plus, optionally, the measured display surface the tracker uses.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub calibration: ScreenCalibration<f32>,
    /// Stable identities, in `calibration.object_points` order. Geometry is not duplicated.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub marker_ids: [String; 6],
}

impl SceneScreen {
    /// A calibrated screen with marker identities derived from its ID.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn from_calibration(id: u8, calibration: ScreenCalibration<f32>) -> Self {
        Self {
            id,
            calibration,
            marker_ids: std::array::from_fn(|index| format!("screen-{id}-marker-{index}")),
        }
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneCamera {
    /// Since: Odyssey Desktop 0.7.0.
    pub id: String,
    /// Since: Odyssey Desktop 0.7.0.
    pub source_id: String,
    /// Since: Odyssey Desktop 0.7.0.
    pub image_size_px: [u32; 2],
    /// Row-major OpenCV pinhole matrix. Pixels have a top-left origin.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub intrinsics: [[f64; 3]; 3],
    /// OpenCV k1, k2, p1, p2, k3 coefficients.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub distortion: [f64; 5],
    /// Image sensor effects applied to renders. `None` renders ideal light.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor: Option<SceneSensor>,
}

/// Renderer sensor model. Rendered linear intensity 1.0 is a full well, so
/// brighter light saturates. Values should come from a measured sensor.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneSensor {
    /// Gaussian point spread, standard deviation in pixels.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub blur_sigma_px: f64,
    /// Electrons at saturation. Sets shot noise relative to signal.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub full_well_electrons: f64,
    /// Since: Odyssey Desktop 0.7.0.
    pub read_noise_electrons: f64,
    /// ADC resolution, 1..=16 bits.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub bit_depth: u8,
    /// Since: Odyssey Desktop 0.7.0.
    pub seed: u64,
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneFrame {
    /// Since: Odyssey Desktop 0.7.0.
    pub id: String,
    /// Since: Odyssey Desktop 0.7.0.
    pub elapsed_us: u64,
    /// Zero means instantaneous exposure, not an unknown exposure duration.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub exposure_us: u64,
    /// Since: Odyssey Desktop 0.7.0.
    pub camera_id: String,
    /// Since: Odyssey Desktop 0.7.0.
    pub camera_to_world: ScenePose,
    /// Geometry visibility labels, independent of dropped/noisy detector observations.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub visible_marker_ids: Vec<String>,
    /// Since: Odyssey Desktop 0.7.0.
    pub truth: SceneTruth,
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenePose {
    /// Since: Odyssey Desktop 0.7.0.
    pub position_m: [f64; 3],
    /// Unit quaternion rotating camera vectors into world space, in x,y,z,w order.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub orientation_xyzw: [f64; 4],
}

/// Independent labels in Odyssey client coordinates: world X, negated world Y and Z.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneTruth {
    /// Since: Odyssey Desktop 0.7.0.
    pub aimpoint: [f64; 2],
    /// Since: Odyssey Desktop 0.7.0.
    pub position_m: Option<[f64; 3]>,
    /// Since: Odyssey Desktop 0.7.0.
    pub screen_id: u32,
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DatasetError {
    /// Since: Odyssey Desktop 0.7.0.
    #[error("unsupported scene schema or missing provenance")]
    Schema,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("scene requires screens, cameras and frames")]
    Empty,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid screen or marker geometry, or duplicate ID")]
    Geometry,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid camera calibration or duplicate camera ID")]
    Camera,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid frame, exposure, pose, truth or label reference")]
    Frame,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid calibrated surface: {0}")]
    Surface(#[from] crate::surface::SurfaceError),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("camera forward ray misses selected target screen")]
    TargetMiss,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("scene asset calibration or tagged geometry differs from scene")]
    AssetIdentity,
}

impl SceneDescription {
    /// Validate labels before handing them to another renderer or model backend.
    ///
    /// # Errors
    ///
    /// Returns an error when scene metadata, geometry, camera data, or frame labels are invalid.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn validate(&self) -> Result<(), DatasetError> {
        self.validate_with_labels(true)
    }

    fn validate_with_labels(&self, labels: bool) -> Result<(), DatasetError> {
        let valid_id = |id: &str| !id.trim().is_empty() && id.len() <= 4096;
        let finite = |values: &[f64]| values.iter().all(|value| value.is_finite());
        if self.schema_version != SCENE_SCHEMA_VERSION || !valid_id(&self.provenance) {
            return Err(DatasetError::Schema);
        }
        if self.screens.is_empty() || self.cameras.is_empty() || self.frames.is_empty() {
            return Err(DatasetError::Empty);
        }
        let mut screen_ids = HashSet::new();
        let mut marker_ids = HashSet::new();
        for screen in &self.screens {
            if !screen_ids.insert(u32::from(screen.id)) {
                return Err(DatasetError::Geometry);
            }
            screen.geometry()?;
            for id in &screen.marker_ids {
                if !marker_ids.insert(id.as_str()) {
                    return Err(DatasetError::Geometry);
                }
            }
        }
        let mut camera_ids = HashSet::new();
        for camera in &self.cameras {
            if !valid_id(&camera.id)
                || !valid_id(&camera.source_id)
                || !camera_ids.insert(camera.id.as_str())
                || camera.image_size_px.contains(&0)
                || !camera.intrinsics.iter().all(|row| finite(row))
                || !finite(&camera.distortion)
                || camera.intrinsics[0][0] <= 0.0
                || camera.intrinsics[1][1] <= 0.0
                || camera.intrinsics[1][0] != 0.0
                || camera.intrinsics[2][0] != 0.0
                || camera.intrinsics[2][1] != 0.0
                || camera.intrinsics[2][2].total_cmp(&1.0).is_ne()
                || camera.sensor.is_some_and(|sensor| {
                    !sensor.blur_sigma_px.is_finite()
                        || sensor.blur_sigma_px < 0.0
                        || !sensor.full_well_electrons.is_finite()
                        || sensor.full_well_electrons < 1.0
                        || !sensor.read_noise_electrons.is_finite()
                        || sensor.read_noise_electrons < 0.0
                        || !(1..=16).contains(&sensor.bit_depth)
                })
            {
                return Err(DatasetError::Camera);
            }
        }
        let mut frame_ids = HashSet::new();
        let mut previous = 0;
        for frame in &self.frames {
            let pose = &frame.camera_to_world;
            let mut visible = HashSet::new();
            if !valid_id(&frame.id)
                || !frame_ids.insert(frame.id.as_str())
                || frame.elapsed_us < previous
                || !camera_ids.contains(frame.camera_id.as_str())
                || frame.elapsed_us.checked_add(frame.exposure_us).is_none()
                || !valid_pose(pose)
                || (labels
                    && (!finite(&frame.truth.aimpoint)
                        || frame
                            .truth
                            .position_m
                            .as_ref()
                            .is_some_and(|position| !finite(position))))
                || !screen_ids.contains(&frame.truth.screen_id)
                || (labels
                    && frame.visible_marker_ids.iter().any(|id| {
                        !marker_ids.contains(id.as_str()) || !visible.insert(id.as_str())
                    }))
            {
                return Err(DatasetError::Frame);
            }
            previous = frame.elapsed_us;
        }
        Ok(())
    }

    /// Recompute geometry-only labels after authoring edits, preserving target screen IDs.
    /// Distorted camera frusta and intervening display surfaces determine marker visibility.
    /// Failure leaves every frame label unchanged. No production tracker is involved.
    ///
    /// # Errors
    ///
    /// Returns an error when scene geometry, camera data, or frame metadata is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn relabel(&mut self) -> Result<(), DatasetError> {
        self.relabel_geometry(None)
    }

    /// Recompute labels with static opaque GLB environment occlusion. Asset metadata
    /// must match authoritative calibration. Failure leaves labels unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when scene geometry is invalid or asset metadata does not match.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn relabel_with_asset(
        &mut self,
        asset: &crate::scene_asset::SceneAsset,
    ) -> Result<(), DatasetError> {
        self.relabel_geometry(Some(asset))
    }

    fn relabel_geometry(
        &mut self,
        asset: Option<&crate::scene_asset::SceneAsset>,
    ) -> Result<(), DatasetError> {
        self.validate_with_labels(false)?;
        if let Some(asset) = asset {
            asset
                .validate_scene(self)
                .map_err(|_identity_mismatch| DatasetError::AssetIdentity)?;
        }
        let geometries = self
            .screens
            .iter()
            .map(SceneScreen::geometry)
            .collect::<Result<Vec<_>, _>>()?;
        let mut labels = Vec::with_capacity(self.frames.len());
        for frame in &self.frames {
            let pose = &frame.camera_to_world;
            let rotation = pose_rotation(pose);
            let direction = rotation.transform_vector(&Vector3::z()).into();
            let target = self
                .screens
                .iter()
                .position(|s| u32::from(s.id) == frame.truth.screen_id)
                .ok_or(DatasetError::Frame)?;
            let hit = geometries[target]
                .raycast(pose.position_m, direction)?
                .ok_or(DatasetError::TargetMiss)?;
            let camera = self
                .cameras
                .iter()
                .find(|camera| camera.id == frame.camera_id)
                .ok_or(DatasetError::Frame)?;
            let mut visible = Vec::new();
            for (screen, geometry) in self.screens.iter().zip(&geometries) {
                for (index, id) in screen.marker_ids.iter().enumerate() {
                    let marker = geometry.marker_world(index).ok_or(DatasetError::Geometry)?;
                    if camera.project_world(pose, marker).is_none()
                        || asset
                            .is_some_and(|asset| asset.occludes_segment(pose.position_m, marker))
                    {
                        continue;
                    }
                    let delta = Vector3::from(marker) - Vector3::from(pose.position_m);
                    let distance = delta.norm();
                    let mut occluded = false;
                    for surface in &geometries {
                        if surface
                            .raycast(pose.position_m, delta.into())?
                            .is_some_and(|hit| hit.distance_m < distance - 1e-6)
                        {
                            occluded = true;
                            break;
                        }
                    }
                    if !occluded {
                        visible.push(id.clone());
                    }
                }
            }
            let [x, y, z] = pose.position_m;
            labels.push((
                SceneTruth {
                    aimpoint: hit.uv,
                    position_m: Some([x, -y, -z]),
                    screen_id: frame.truth.screen_id,
                },
                visible,
            ));
        }
        for (frame, (truth, visible)) in self.frames.iter_mut().zip(labels) {
            frame.truth = truth;
            frame.visible_marker_ids = visible;
        }
        Ok(())
    }
}

fn valid_pose(pose: &ScenePose) -> bool {
    pose.position_m
        .iter()
        .chain(&pose.orientation_xyzw)
        .all(|v| v.is_finite())
        && (pose.orientation_xyzw.iter().map(|v| v * v).sum::<f64>() - 1.0).abs() <= 1e-6
}

fn pose_rotation(pose: &ScenePose) -> UnitQuaternion<f64> {
    let [x, y, z, w] = pose.orientation_xyzw;
    UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z))
}

impl SceneCamera {
    /// OpenCV projection including k1,k2,p1,p2,k3 distortion. None means behind/outside.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn project_world(&self, pose: &ScenePose, point_world_m: [f64; 3]) -> Option<[f64; 2]> {
        project_world(
            self.image_size_px,
            &self.intrinsics,
            &self.distortion,
            pose,
            point_world_m,
        )
    }
}

/// [`SceneCamera::project_world`] for optics that are not part of a scene, such
/// as a simulated unit's own camera configuration.
///
/// Since: Odyssey Desktop 0.7.5.
pub fn project_world(
    image_size_px: [u32; 2],
    intrinsics: &[[f64; 3]; 3],
    distortion: &[f64; 5],
    pose: &ScenePose,
    point_world_m: [f64; 3],
) -> Option<[f64; 2]> {
    let local = pose_rotation(pose)
        .inverse_transform_vector(&(Vector3::from(point_world_m) - Vector3::from(pose.position_m)));
    if local.z <= 0.0 {
        return None;
    }
    let x = local.x / local.z;
    let y = local.y / local.z;
    let r2 = x * x + y * y;
    let [k1, k2, p1, p2, k3] = *distortion;
    let radial = 1.0 + r2 * (k1 + r2 * (k2 + r2 * k3));
    let dx = x * radial + 2.0 * p1 * x * y + p2 * (r2 + 2.0 * x * x);
    let dy = y * radial + p1 * (r2 + 2.0 * y * y) + 2.0 * p2 * x * y;
    let u = intrinsics[0][0] * dx + intrinsics[0][1] * dy + intrinsics[0][2];
    let v = intrinsics[1][1] * dy + intrinsics[1][2];
    (u.is_finite()
        && v.is_finite()
        && (0.0..f64::from(image_size_px[0])).contains(&u)
        && (0.0..f64::from(image_size_px[1])).contains(&v))
    .then_some([u, v])
}
