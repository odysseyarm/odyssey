//! Hand-off to the Blender and Rerun tools in `tools/tracking_scene`: the stage
//! writes the scene they read, and takes back a display edited in Blender.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use odyssey_sim::dataset::SceneDescription;
use odyssey_stage::display::Display;
use serde::{Deserialize, Serialize};

use crate::api::{StageReply, stage_reply};
use crate::app::App;
use crate::error::ApiError;

#[derive(Deserialize)]
pub(crate) struct ExportRequest {
    device: String,
    fps: Option<f64>,
}

#[derive(Serialize)]
pub(crate) struct Step {
    title: &'static str,
    command: String,
}

#[derive(Serialize)]
pub(crate) struct BlenderExport {
    scene_path: String,
    steps: Vec<Step>,
}

pub(crate) fn export_dir() -> Result<PathBuf, ApiError> {
    let dir = std::env::temp_dir().join("odyssey-stage");
    std::fs::create_dir_all(&dir)
        .map_err(|error| ApiError::Internal(format!("cannot create {}: {error}", dir.display())))?;
    Ok(dir)
}

fn step(title: &'static str, command: impl Into<String>) -> Step {
    Step {
        title,
        command: command.into(),
    }
}

/// Write the staged run as a scene file and list the commands that turn it
/// into a `.blend`, render it, view it in Rerun, and bring edits back.
pub(crate) async fn export_blender(
    State(app): State<Arc<App>>,
    Json(request): Json<ExportRequest>,
) -> Result<Json<BlenderExport>, ApiError> {
    let scene = app
        .compiled()
        .scene(&request.device, request.fps.unwrap_or(30.0))?;
    let dir = export_dir()?;
    let name = format!("staged-{}", request.device);
    let scene_path = dir.join(format!("{name}.json"));
    let json =
        serde_json::to_vec_pretty(&scene).map_err(|error| ApiError::Internal(error.to_string()))?;
    std::fs::write(&scene_path, json).map_err(|error| {
        ApiError::Internal(format!("cannot write {}: {error}", scene_path.display()))
    })?;
    let scene = scene_path.display();
    let blend = dir.join(format!("{name}.blend")).display().to_string();
    let edited = dir
        .join(format!("{name}-edited.json"))
        .display()
        .to_string();
    let render = dir.join(format!("{name}-render")).display().to_string();
    let rrd = dir.join(format!("{name}.rrd")).display().to_string();
    let blender = "blender -b --python-exit-code 1 --python tools/tracking_scene/blender_scene.py -- --validator \"$VALIDATOR\"";
    Ok(Json(BlenderExport {
        scene_path: scene.to_string(),
        steps: vec![
            step(
                "Open the tool shell (once, from the repository root)",
                "nix build .#tracking && export VALIDATOR=\"$PWD/result/bin/tracking-regression\" && nix develop .#tracking",
            ),
            step(
                "Build the Blender file",
                format!("{blender} build {scene} {blend}"),
            ),
            step(
                "Edit the display and markers in Blender",
                format!("blender {blend}"),
            ),
            step(
                "Export your edits, then import the result in the stage",
                format!("{blender} export {blend} {edited}"),
            ),
            step(
                "Render camera images",
                format!("{blender} render {blend} {render}"),
            ),
            step(
                "Inspect in Rerun",
                format!(
                    "python3 tools/tracking_scene/rerun_export.py {scene} {rrd} --validator \"$VALIDATOR\" --self-check && rerun {rrd}"
                ),
            ),
        ],
    }))
}

/// Adopt the display of a scene file, such as the export of a Blender edit.
pub(crate) async fn import_scene(
    State(app): State<Arc<App>>,
    Json(scene): Json<SceneDescription>,
) -> Result<Json<StageReply>, ApiError> {
    let mut stage = app.compiled().stage().clone();
    stage.display = Display::from_scene(&scene)?;
    let compiled = app.set_stage(&stage)?;
    Ok(Json(stage_reply(&compiled)?))
}
