//! Static self-contained glTF2 assets. Calibration JSON remains authoritative.
//! Tagged display/marker/camera meshes never replace calibrated geometry.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use nalgebra::{Matrix4, Vector3, Vector4};
use odyssey_common::triangle_hit;
use serde_json::Value;

use crate::dataset::SceneDescription;

/// Since: Odyssey Desktop 0.7.0.
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;
const MAX_NODES: usize = 4096;
const MAX_TRIANGLES: usize = 1_000_000;
const MAX_ACCESSOR_ITEMS: usize = 3_000_000;

/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    /// Since: Odyssey Desktop 0.7.0.
    #[error("cannot read scene asset: {0}")]
    Io(#[from] std::io::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("cannot parse glTF2 asset: {0}")]
    Gltf(#[from] gltf::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("cannot parse calibration metadata: {0}")]
    Json(#[from] serde_json::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid embedded calibration scene: {0}")]
    Scene(#[from] crate::dataset::DatasetError),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid glTF2 binary asset: {0}")]
    Invalid(String),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("scene asset exceeds bounded byte/node/triangle/accessor limits or buffer bounds")]
    Bounds,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("unsupported scene asset: {0}")]
    Unsupported(&'static str),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("asset calibration/identity mismatch: {0}")]
    Identity(&'static str),
}

#[derive(Debug)]
struct TaggedNode {
    kind: String,
    extras: Value,
    origin_world_m: [f64; 3],
    screen_vertices: Vec<([f64; 3], [f64; 2])>,
}

/// Validated world-space environment triangles. glTF Y-up coordinates map to
/// Odyssey as [x,-z,y], matching Blender's default Y-up glTF export.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug)]
pub struct SceneAsset {
    triangles: Vec<[Vector3<f64>; 3]>,
    tagged: Vec<TaggedNode>,
    embedded_scene: Option<SceneDescription>,
}

impl SceneAsset {
    /// Load and parse a GLB scene asset.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or its GLB data is unsupported or invalid.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AssetError> {
        let file = File::open(path)?;
        let mut bytes = Vec::new();
        file.take(MAX_ASSET_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        Self::from_glb(&bytes)
    }

    /// Parse embedded GLB only. URLs, dynamics, sparse accessors, non-triangle
    /// primitives and nonopaque environment materials fail instead of approximating.
    ///
    /// # Errors
    ///
    /// Returns an error when GLB data is malformed, unsupported, or exceeds size bounds.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn from_glb(bytes: &[u8]) -> Result<Self, AssetError> {
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(AssetError::Bounds);
        }
        // Guard gltf-rs's header-length subtraction before delegating parsing.
        if bytes.len() < 12
            || &bytes[..4] != b"glTF"
            || u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) != 2
            || u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize
                != bytes.len()
        {
            return Err(AssetError::Invalid(
                "requires complete GLB version 2".into(),
            ));
        }
        let gltf::Gltf { document, blob } = gltf::Gltf::from_slice_without_validation(bytes)?;
        let root = document.into_json();
        // gltf-json's primitive hook indexes POSITION before reporting bad references.
        if root
            .meshes
            .iter()
            .flat_map(|mesh| &mesh.primitives)
            .flat_map(|primitive| primitive.attributes.values())
            .any(|index| index.value() >= root.accessors.len())
        {
            return Err(AssetError::Bounds);
        }
        let gltf = gltf::Gltf {
            document: gltf::Document::from_json(root)?,
            blob,
        };
        let document = &gltf.document;
        if document.nodes().len() > MAX_NODES {
            return Err(AssetError::Bounds);
        }
        if document.animations().next().is_some()
            || document.skins().next().is_some()
            || document
                .nodes()
                .any(|n| n.skin().is_some() || n.weights().is_some())
            || document.meshes().any(|m| {
                m.weights().is_some() || m.primitives().any(|p| p.morph_targets().next().is_some())
            })
        {
            return Err(AssetError::Unsupported(
                "animations, skins and morph targets",
            ));
        }
        if document.extensions_used().any(|name| {
            !matches!(
                name,
                "KHR_materials_unlit"
                    | "KHR_materials_emissive_strength"
                    | "KHR_texture_transform"
                    | "KHR_lights_punctual"
            )
        }) {
            return Err(AssetError::Unsupported(
                "glTF extension affecting unsupported geometry/material behavior",
            ));
        }
        let blob = gltf.blob.as_deref().unwrap_or(&[]);
        for buffer in document.buffers() {
            if !matches!(buffer.source(), gltf::buffer::Source::Bin) {
                return Err(AssetError::Unsupported("external or data-URI buffers"));
            }
            if buffer.length() > blob.len() {
                return Err(AssetError::Bounds);
            }
        }
        for image in document.images() {
            if !matches!(image.source(), gltf::image::Source::View { .. }) {
                return Err(AssetError::Unsupported("external or data-URI images"));
            }
        }
        for view in document.views() {
            if view
                .offset()
                .checked_add(view.length())
                .is_none_or(|end| end > view.buffer().length() || end > blob.len())
            {
                return Err(AssetError::Bounds);
            }
        }
        for accessor in document.accessors() {
            if accessor.sparse().is_some() {
                return Err(AssetError::Unsupported("sparse accessors"));
            }
            let view = accessor.view().ok_or(AssetError::Bounds)?;
            let stride = view.stride().unwrap_or(accessor.size());
            let extent = accessor
                .count()
                .checked_sub(1)
                .and_then(|n| n.checked_mul(stride))
                .and_then(|n| n.checked_add(accessor.size()))
                .and_then(|n| n.checked_add(accessor.offset()));
            if accessor.count() > MAX_ACCESSOR_ITEMS
                || stride < accessor.size()
                || extent.is_none_or(|end| end > view.length())
            {
                return Err(AssetError::Bounds);
            }
        }
        let scene = document
            .default_scene()
            .or_else(|| {
                if document.scenes().len() == 1 {
                    document.scenes().next()
                } else {
                    None
                }
            })
            .ok_or(AssetError::Unsupported(
                "requires one selected default scene",
            ))?;
        let mut asset = Self {
            triangles: Vec::new(),
            tagged: Vec::new(),
            embedded_scene: None,
        };
        let mut visited = vec![false; document.nodes().len()];
        let mut processed_vertices = 0_usize;
        let mut processed_triangles = 0_usize;
        let mut stack = scene
            .nodes()
            .map(|node| (node, Matrix4::<f64>::identity(), false, None))
            .collect::<Vec<_>>();
        while let Some((node, parent, inherited_tag, inherited_screen)) = stack.pop() {
            if visited[node.index()] {
                return Err(AssetError::Invalid(
                    "cyclic or multiply parented node hierarchy".into(),
                ));
            }
            visited[node.index()] = true;
            let matrix = node.transform().matrix();
            let local = Matrix4::from_fn(|row, column| f64::from(matrix[column][row]));
            let transform = parent * local;
            let determinant = transform.fixed_view::<3, 3>(0, 0).determinant();
            if !transform.iter().all(|v| v.is_finite())
                || transform
                    .row(3)
                    .iter()
                    .zip([0.0, 0.0, 0.0, 1.0])
                    .any(|(a, b)| (*a - b).abs() > 1e-8)
                || !determinant.is_finite()
                || determinant == 0.0
            {
                return Err(AssetError::Invalid(
                    "nonfinite, projective or singular node transform".into(),
                ));
            }
            let extras: Value = node
                .extras()
                .as_ref()
                .map(|raw| serde_json::from_str(raw.get()))
                .transpose()?
                .unwrap_or(Value::Null);
            if let Some(value) = extras.get("odyssey_scene_json") {
                let text = value
                    .as_str()
                    .ok_or(AssetError::Identity("odyssey_scene_json must be JSON text"))?;
                let scene: SceneDescription = serde_json::from_str(text)?;
                scene.validate()?;
                if asset.embedded_scene.is_some() {
                    return Err(AssetError::Identity(
                        "duplicate embedded calibration sidecar",
                    ));
                }
                asset.embedded_scene = Some(scene);
            }
            let kind = extras.get("odyssey_kind").and_then(Value::as_str);
            let tagged = kind.is_some_and(|kind| matches!(kind, "screen" | "marker" | "camera"));
            let excluded = inherited_tag || tagged;
            let screen_owner = if let Some(kind) =
                kind.filter(|kind| matches!(*kind, "screen" | "marker" | "camera"))
            {
                let index = asset.tagged.len();
                asset.tagged.push(TaggedNode {
                    kind: kind.into(),
                    extras: extras.clone(),
                    origin_world_m: transform_point(&transform, [0.0; 3])?,
                    screen_vertices: Vec::new(),
                });
                (kind == "screen").then_some(index)
            } else {
                inherited_screen
            };
            if let Some(mesh) = node.mesh() {
                for primitive in mesh.primitives() {
                    if primitive.mode() != gltf::mesh::Mode::Triangles {
                        return Err(AssetError::Unsupported("non-triangle primitives"));
                    }
                    if !excluded
                        && primitive.material().alpha_mode() != gltf::material::AlphaMode::Opaque
                    {
                        return Err(AssetError::Unsupported("nonopaque environment materials"));
                    }
                    let position_accessor = primitive
                        .get(&gltf::Semantic::Positions)
                        .ok_or(AssetError::Invalid("missing POSITION accessor".into()))?;
                    if position_accessor.data_type() != gltf::accessor::DataType::F32
                        || position_accessor.dimensions() != gltf::accessor::Dimensions::Vec3
                        || position_accessor.normalized()
                    {
                        return Err(AssetError::Unsupported(
                            "POSITION must be unnormalized float32 VEC3",
                        ));
                    }
                    processed_vertices = processed_vertices
                        .checked_add(position_accessor.count())
                        .ok_or(AssetError::Bounds)?;
                    let count = primitive
                        .indices()
                        .map_or(position_accessor.count(), |a| a.count());
                    processed_triangles = processed_triangles
                        .checked_add(count / 3)
                        .ok_or(AssetError::Bounds)?;
                    if count % 3 != 0
                        || processed_vertices > MAX_ACCESSOR_ITEMS
                        || processed_triangles > MAX_TRIANGLES
                    {
                        return Err(AssetError::Bounds);
                    }
                    let reader = primitive.reader(|_| Some(blob));
                    let positions = reader
                        .read_positions()
                        .ok_or(AssetError::Invalid("missing float32 POSITION data".into()))?
                        .map(|p| transform_point(&transform, p.map(f64::from)))
                        .collect::<Result<Vec<_>, _>>()?;
                    let indices = if primitive.indices().is_some() {
                        reader
                            .read_indices()
                            .ok_or(AssetError::Invalid("invalid index accessor".into()))?
                            .into_u32()
                            .map(|i| i as usize)
                            .collect::<Vec<_>>()
                    } else {
                        (0..positions.len()).collect()
                    };
                    if indices.len() != count || indices.iter().any(|i| *i >= positions.len()) {
                        return Err(AssetError::Bounds);
                    }
                    if let Some(owner) = screen_owner {
                        let texcoords = reader
                            .read_tex_coords(0)
                            .ok_or(AssetError::Identity(
                                "tagged screen requires canonical TEXCOORD_0",
                            ))?
                            .into_f32()
                            .collect::<Vec<_>>();
                        if texcoords.len() != positions.len() {
                            return Err(AssetError::Identity("screen UV/POSITION counts differ"));
                        }
                        asset.tagged[owner].screen_vertices.extend(
                            positions
                                .iter()
                                .copied()
                                .zip(texcoords.into_iter().map(|uv| uv.map(f64::from))),
                        );
                    }
                    if !excluded {
                        for indices in indices.as_chunks::<3>().0 {
                            let triangle = [indices[0], indices[1], indices[2]]
                                .map(|i| Vector3::from(positions[i]));
                            let area = (triangle[1] - triangle[0])
                                .cross(&(triangle[2] - triangle[0]))
                                .norm_squared();
                            if !area.is_finite() || area <= 1e-24 {
                                return Err(AssetError::Invalid(
                                    "degenerate environment triangle".into(),
                                ));
                            }
                            asset.triangles.push(triangle);
                        }
                    }
                }
            }
            for child in node.children() {
                stack.push((child, transform, excluded, screen_owner));
            }
        }
        if let Some(scene) = &asset.embedded_scene {
            asset.validate_scene(scene)?;
        }
        Ok(asset)
    }

    /// Validate metadata available in asset against authoritative calibration labels.
    ///
    /// # Errors
    ///
    /// Returns an error when embedded calibration or tagged geometry differs from scene.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn validate_scene(&self, scene: &SceneDescription) -> Result<(), AssetError> {
        if let Some(embedded) = &self.embedded_scene
            && (embedded.cameras != scene.cameras
                || embedded.screens.len() != scene.screens.len()
                || !embedded.screens.iter().zip(&scene.screens).all(|(a, b)| {
                    a.id == b.id
                        && a.marker_ids == b.marker_ids
                        && a.calibration.surface == b.calibration.surface
                        && a.calibration.rotation == b.calibration.rotation
                        && a.calibration.homography == b.calibration.homography
                        && a.calibration.object_points == b.calibration.object_points
                }))
        {
            return Err(AssetError::Identity(
                "embedded screen/camera calibration differs",
            ));
        }
        let mut identities = std::collections::HashSet::new();
        for tagged in &self.tagged {
            match tagged.kind.as_str() {
                "screen" => {
                    let id = tagged
                        .extras
                        .get("screen_id")
                        .and_then(Value::as_u64)
                        .ok_or(AssetError::Identity("screen missing screen_id"))?;
                    let screen = scene
                        .screens
                        .iter()
                        .find(|s| u64::from(s.id) == id)
                        .ok_or(AssetError::Identity("unknown screen_id"))?;
                    if !identities.insert(format!("screen:{id}")) {
                        return Err(AssetError::Identity("duplicate screen_id"));
                    }
                    let geometry = screen.geometry().map_err(|_invalid_screen| {
                        AssetError::Identity("invalid canonical screen")
                    })?;
                    let mut corners = [false; 4];
                    for (point, uv) in &tagged.screen_vertices {
                        let expected = geometry.point_at_uv(*uv).map_err(|_invalid_uv| {
                            AssetError::Identity("screen UV outside canonical [0,1]")
                        })?;
                        if (Vector3::from(*point) - Vector3::from(expected)).norm() > 1e-4 {
                            return Err(AssetError::Identity(
                                "screen mesh/UV differs from calibrated surface",
                            ));
                        }
                        for (present, corner) in
                            corners
                                .iter_mut()
                                .zip([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]])
                        {
                            *present |= (uv[0] - corner[0]).abs() < 1e-6
                                && (uv[1] - corner[1]).abs() < 1e-6;
                        }
                    }
                    if !corners.into_iter().all(|present| present) {
                        return Err(AssetError::Identity(
                            "screen mesh missing canonical UV corners",
                        ));
                    }
                }
                "marker" => {
                    let id = tagged
                        .extras
                        .get("marker_id")
                        .and_then(Value::as_str)
                        .ok_or(AssetError::Identity("marker missing marker_id"))?;
                    let screen_id = tagged.extras.get("screen_id").and_then(Value::as_u64);
                    let screen = scene
                        .screens
                        .iter()
                        .find(|s| {
                            Some(u64::from(s.id)) == screen_id
                                && s.marker_ids.iter().any(|marker| marker == id)
                        })
                        .ok_or(AssetError::Identity("unknown marker/screen identity"))?;
                    let index = screen
                        .marker_ids
                        .iter()
                        .position(|marker| marker == id)
                        .ok_or(AssetError::Identity("missing canonical marker"))?;
                    let world = screen
                        .geometry()
                        .map_err(|_invalid_calibration| {
                            AssetError::Identity("invalid canonical calibration")
                        })?
                        .marker_world(index)
                        .ok_or(AssetError::Identity("missing canonical marker"))?;
                    if !identities.insert(format!("marker:{id}"))
                        || (Vector3::from(world) - Vector3::from(tagged.origin_world_m)).norm()
                            > 1e-4
                    {
                        return Err(AssetError::Identity(
                            "duplicate or displaced canonical marker",
                        ));
                    }
                }
                "camera" => {
                    let id = tagged
                        .extras
                        .get("camera_id")
                        .and_then(Value::as_str)
                        .ok_or(AssetError::Identity("camera missing camera_id"))?;
                    if !scene.cameras.iter().any(|camera| camera.id == id)
                        || !identities.insert(format!("camera:{id}"))
                    {
                        return Err(AssetError::Identity("unknown or duplicate camera_id"));
                    }
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    /// True when an opaque environment triangle lies strictly before endpoint.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn occludes_segment(&self, origin_world_m: [f64; 3], endpoint_world_m: [f64; 3]) -> bool {
        let origin = Vector3::from(origin_world_m);
        let delta = Vector3::from(endpoint_world_m) - origin;
        let distance = delta.norm();
        if !distance.is_finite() || distance <= 1e-6 {
            return false;
        }
        let direction = delta / distance;
        self.triangles.iter().any(|[a, b, c]| {
            triangle_hit(origin, direction, *a, *b, *c)
                .is_some_and(|(hit, _)| hit < distance - 1e-6)
        })
    }
}

fn transform_point(transform: &Matrix4<f64>, point: [f64; 3]) -> Result<[f64; 3], AssetError> {
    let p = transform * Vector4::new(point[0], point[1], point[2], 1.0);
    let world = [p.x, -p.z, p.y];
    if world.iter().all(|v| v.is_finite()) {
        Ok(world)
    } else {
        Err(AssetError::Invalid("nonfinite mesh position".into()))
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use serde_json::json;

    use super::*;
    use crate::dataset::{
        DatasetError, SCENE_SCHEMA_VERSION, SceneCamera, SceneCoordinateBasis, SceneFrame,
        ScenePose, SceneScreen, SceneTruth,
    };

    fn fixture(edit: impl FnOnce(&mut Value, &mut Vec<u8>)) -> Vec<u8> {
        let mut bin = [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        bin.extend([0_u16, 1, 2].into_iter().flat_map(u16::to_le_bytes));
        let mut document = json!({
            "asset":{"version":"2.0"}, "scene":0, "scenes":[{"nodes":[0]}],
            "nodes":[{"translation":[0,-1,0],"children":[1]},{"mesh":0}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1,"mode":4}]}],
            "buffers":[{"byteLength":42}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":6}],
            "accessors":[
                {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,-1],"max":[1,0,0]},
                {"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}
            ]
        });
        edit(&mut document, &mut bin);
        let glb = gltf::binary::Glb {
            header: gltf::binary::Header {
                magic: *b"glTF",
                version: 2,
                length: 0,
            },
            json: Cow::Owned(serde_json::to_vec(&document).unwrap()),
            bin: Some(Cow::Owned(bin)),
        };
        let mut bytes = Vec::new();
        glb.to_writer(&mut bytes).unwrap();
        bytes
    }

    fn scene() -> SceneDescription {
        SceneDescription {
            schema_version: SCENE_SCHEMA_VERSION,
            provenance: "Independent static-GLB occlusion fixture".into(),
            coordinate_basis: SceneCoordinateBasis::ScreenXRightYDownZForward,
            screens: vec![SceneScreen {
                id: 0,
                calibration: crate::synthetic_screen_calibration(),
                marker_ids: std::array::from_fn(|i| format!("marker-{i}")),
            }],
            cameras: vec![SceneCamera {
                id: "camera".into(),
                source_id: "source".into(),
                image_size_px: [640, 480],
                intrinsics: [[500.0, 0.0, 320.0], [0.0, 500.0, 240.0], [0.0, 0.0, 1.0]],
                distortion: [0.0; 5],
                sensor: None,
            }],
            frames: vec![SceneFrame {
                id: "frame".into(),
                elapsed_us: 0,
                exposure_us: 0,
                camera_id: "camera".into(),
                camera_to_world: ScenePose {
                    position_m: [0.6, 0.3375, -2.0],
                    orientation_xyzw: [0.0, 0.0, 0.0, 1.0],
                },
                visible_marker_ids: vec![],
                truth: SceneTruth {
                    aimpoint: [0.5; 2],
                    position_m: None,
                    screen_id: 0,
                },
            }],
        }
    }

    #[test]
    fn static_glb_hierarchy_axis_conversion_and_segment_endpoints() {
        let bytes = fixture(|json, _| json["nodes"][1]["translation"] = json!([2, 0, 0]));
        let asset = SceneAsset::from_glb(&bytes).unwrap();
        assert!(asset.occludes_segment([2.2, 0.2, -2.0], [2.2, 0.2, 0.0]));
        assert!(!asset.occludes_segment([2.9, 0.9, -2.0], [2.9, 0.9, 0.0]));
        assert!(!asset.occludes_segment([2.2, 0.2, -2.0], [2.2, 0.2, -1.0]));
        assert!(!asset.occludes_segment([2.2, 0.2, -2.0], [2.2, 0.2, -1.5]));
        assert!(asset.occludes_segment([2.2, 0.2, 0.0], [2.2, 0.2, -2.0]));
    }

    #[test]
    fn canonical_tagged_subtrees_are_not_environment_occluders() {
        let bytes = fixture(|json, _| {
            json["nodes"][0]["translation"] = json!([0, 0, 0]);
            json["nodes"][1]["translation"] = json!([0, -1, 0]);
            json["nodes"][0]["extras"] = json!({"odyssey_kind":"camera","camera_id":"camera"});
        });
        let asset = SceneAsset::from_glb(&bytes).unwrap();
        asset.validate_scene(&scene()).unwrap();
        assert!(!asset.occludes_segment([0.2, 0.2, -2.0], [0.2, 0.2, 0.0]));
        let mut invalid = scene();
        invalid.cameras[0].id = "missing-camera".into();
        assert!(matches!(
            asset.validate_scene(&invalid),
            Err(AssetError::Identity(_))
        ));
    }

    #[test]
    fn glb_environment_relabels_visibility_without_changing_surface_truth() {
        let mut scene = scene();
        scene.relabel().unwrap();
        let expected = scene.frames[0].truth.aimpoint;
        assert!(
            scene.frames[0]
                .visible_marker_ids
                .contains(&"marker-0".into())
        );
        let asset = SceneAsset::from_glb(&fixture(|_, _| {})).unwrap();
        scene.relabel_with_asset(&asset).unwrap();
        assert!(
            !scene.frames[0]
                .visible_marker_ids
                .contains(&"marker-0".into())
        );
        assert_eq!(scene.frames[0].truth.aimpoint, expected);
        let distractor = SceneAsset::from_glb(&fixture(|json, _| {
            json["nodes"][1]["translation"] = json!([5, 0, 0]);
            json["materials"] = json!([{"emissiveFactor":[1,1,1]}]);
            json["meshes"][0]["primitives"][0]["material"] = json!(0);
        }))
        .unwrap();
        scene.relabel_with_asset(&distractor).unwrap();
        assert!(
            scene.frames[0]
                .visible_marker_ids
                .contains(&"marker-0".into())
        );
        scene.frames[0].camera_to_world.position_m[0] = 99.0;
        let before = scene.frames[0].visible_marker_ids.clone();
        assert_eq!(
            scene.relabel_with_asset(&asset),
            Err(DatasetError::TargetMiss)
        );
        assert_eq!(scene.frames[0].visible_marker_ids, before);
    }

    #[test]
    fn external_resources_dynamic_geometry_and_transparency_are_explicit_errors() {
        let edits: [fn(&mut Value); 6] = [
            |json: &mut Value| json["buffers"][0]["uri"] = json!("remote.bin"),
            |json: &mut Value| {
                json["images"] = json!([{"uri":"https://example.invalid/texture.png"}]);
            },
            |json: &mut Value| {
                json["meshes"][0]["primitives"][0]["targets"] = json!([{"POSITION":0}]);
            },
            |json: &mut Value| {
                json["materials"] = json!([{"alphaMode":"BLEND"}]);
                json["meshes"][0]["primitives"][0]["material"] = json!(0);
            },
            |json: &mut Value| json["meshes"][0]["primitives"][0]["mode"] = json!(1),
            |json: &mut Value| json["extensionsUsed"] = json!(["KHR_materials_transmission"]),
        ];
        for edit in edits {
            let bytes = fixture(|json, _| edit(json));
            assert!(matches!(
                SceneAsset::from_glb(&bytes),
                Err(AssetError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn malformed_accessor_ranges_indices_and_headers_are_bounded_errors() {
        let short_header = [b'g', b'l', b'T', b'F', 2, 0, 0, 0, 4, 0, 0, 0];
        assert!(matches!(
            SceneAsset::from_glb(&short_header),
            Err(AssetError::Invalid(_))
        ));
        let bytes = fixture(|json, _| json["accessors"][0]["count"] = json!(u64::MAX));
        SceneAsset::from_glb(&bytes).unwrap_err();
        let bytes = fixture(|_, bin| bin[40..42].copy_from_slice(&99_u16.to_le_bytes()));
        assert!(matches!(
            SceneAsset::from_glb(&bytes),
            Err(AssetError::Bounds)
        ));
        let bytes = fixture(|json, _| json["bufferViews"][0]["byteLength"] = json!(usize::MAX));
        SceneAsset::from_glb(&bytes).unwrap_err();
    }

    #[test]
    fn embedded_calibration_and_tagged_marker_positions_cannot_disagree() {
        let scene = scene();
        let bytes = fixture(|json, _| {
            json["nodes"][0]["extras"] =
                json!({"odyssey_scene_json":serde_json::to_string(&scene).unwrap()});
        });
        let asset = SceneAsset::from_glb(&bytes).unwrap();
        let mut changed = scene.clone();
        changed.screens[0].calibration.homography[(0, 0)] *= 1.01;
        assert!(matches!(
            asset.validate_scene(&changed),
            Err(AssetError::Identity(_))
        ));
        let bytes = fixture(|json, _| {
            json["nodes"][1]["extras"] =
                json!({"odyssey_kind":"marker","screen_id":0,"marker_id":"marker-0"});
        });
        let asset = SceneAsset::from_glb(&bytes).unwrap();
        assert!(matches!(
            asset.validate_scene(&scene),
            Err(AssetError::Identity(_))
        ));
    }

    #[test]
    fn tagged_screen_vertices_and_asymmetric_uv_anchors_match_calibration() {
        let mut scene = scene();
        scene.screens[0].calibration.homography = nalgebra::Matrix3::identity();
        let screen_glb = |flip_top_left_v| {
            fixture(|json, bin| {
                json["nodes"][0]["translation"] = json!([0, 0, 0]);
                json["nodes"][0]["extras"] = json!({
                    "odyssey_kind":"screen", "screen_id":0,
                    "odyssey_scene_json":serde_json::to_string(&scene).unwrap()
                });
                bin.clear();
                bin.extend(
                    [
                        0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 1.0, 0.0, -1.0,
                    ]
                    .into_iter()
                    .flat_map(f32::to_le_bytes),
                );
                bin.extend(
                    [
                        0_f32,
                        if flip_top_left_v { 1.0 } else { 0.0 },
                        1.0,
                        0.0,
                        0.0,
                        1.0,
                        1.0,
                        1.0,
                    ]
                    .into_iter()
                    .flat_map(f32::to_le_bytes),
                );
                bin.extend(
                    [0_u16, 1, 3, 0, 3, 2]
                        .into_iter()
                        .flat_map(u16::to_le_bytes),
                );
                json["buffers"][0]["byteLength"] = json!(92);
                json["bufferViews"] = json!([
                    {"buffer":0,"byteOffset":0,"byteLength":48},
                    {"buffer":0,"byteOffset":80,"byteLength":12},
                    {"buffer":0,"byteOffset":48,"byteLength":32}
                ]);
                json["accessors"][0]["count"] = json!(4);
                json["accessors"][1]["count"] = json!(6);
                json["accessors"].as_array_mut().unwrap().push(json!({
                    "bufferView":2,"componentType":5126,"count":4,"type":"VEC2"
                }));
                json["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = json!(2);
            })
        };
        let asset = SceneAsset::from_glb(&screen_glb(false)).unwrap();
        asset.validate_scene(&scene).unwrap();
        assert!(matches!(
            SceneAsset::from_glb(&screen_glb(true)),
            Err(AssetError::Identity(_))
        ));
        let mut moved = scene.clone();
        moved.screens[0].calibration.surface = Some(odyssey_common::ScreenSurface {
            columns: 2,
            rows: 2,
            positions_m: vec![
                [0.0, 0.0, 0.1],
                [1.0, 0.0, 0.1],
                [0.0, 1.0, 0.1],
                [1.0, 1.0, 0.1],
            ],
        });
        assert!(matches!(
            asset.validate_scene(&moved),
            Err(AssetError::Identity(_))
        ));
    }

    #[test]
    fn parser_errors_preserve_underlying_error_source() {
        let glb = gltf::binary::Glb {
            header: gltf::binary::Header {
                magic: *b"glTF",
                version: 2,
                length: 0,
            },
            json: Cow::Borrowed(b"{"),
            bin: None,
        };
        let mut bytes = Vec::new();
        glb.to_writer(&mut bytes).unwrap();
        let error = SceneAsset::from_glb(&bytes).unwrap_err();
        assert!(matches!(&error, AssetError::Gltf(_)));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn malformed_attribute_references_fail_before_upstream_validation_hook() {
        for semantic in ["POSITION", "NORMAL", "TEXCOORD_0"] {
            let bytes = fixture(|json, _| {
                json["meshes"][0]["primitives"][0]["attributes"][semantic] = json!(99);
            });
            assert!(matches!(
                SceneAsset::from_glb(&bytes),
                Err(AssetError::Bounds)
            ));
        }
    }
}
