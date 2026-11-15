use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use odyssey_sim::{Command, Target};
use odyssey_stage::display::Display;
use odyssey_stage::pattern::{Button, PATTERN_NAMES, Pattern};
use odyssey_stage::radio::ENVIRONMENTS;
use odyssey_stage::{DISPLAY_PRESET_NAMES, Stage, display_preset};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

use crate::app::{App, Derived, Message};
use crate::error::ApiError;
use crate::evaluate::{Evaluation, evaluate};

type Shared = State<Arc<App>>;

const INDEX: &str = include_str!("../web/index.html");
const SCRIPT: &str = include_str!("../web/app.js");
const STYLE: &str = include_str!("../web/style.css");

fn asset(
    app: &Arc<App>,
    name: &str,
    embedded: &'static str,
    content_type: &'static str,
) -> Response {
    let body = match &app.web_dir {
        Some(dir) => std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|error| format!("{name}: {error}")),
        None => embedded.to_owned(),
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

pub(crate) fn router(app: Arc<App>, port: u16) -> Router {
    Router::new()
        .route(
            "/",
            get(|State(app): Shared| async move {
                asset(&app, "index.html", INDEX, "text/html; charset=utf-8")
            }),
        )
        .route(
            "/app.js",
            get(
                |State(app): Shared| async move { asset(&app, "app.js", SCRIPT, "text/javascript") },
            ),
        )
        .route(
            "/style.css",
            get(|State(app): Shared| async move { asset(&app, "style.css", STYLE, "text/css") }),
        )
        .route("/api/state", get(state))
        .route("/api/stage", axum::routing::put(put_stage))
        .route("/api/run", post(run))
        .route("/api/press", post(press))
        .route("/api/sim", post(sim_command))
        .route("/api/hub/display", post(adopt_hub_display))
        .route("/api/evaluate", post(evaluate_run))
        .route("/api/export/scene", get(export_scene))
        .route("/api/export/blender", post(crate::blender::export_blender))
        .route("/api/import/scene", post(crate::blender::import_scene))
        .route("/api/events", get(events))
        .layer(axum::middleware::from_fn(move |request, next| {
            local_only(port, request, next)
        }))
        .with_state(app)
}

/// Refuse requests that did not come from a page this server served: a
/// foreign site could otherwise reach a loopback server through the browser
/// (DNS rebinding, cross-site form posts).
async fn local_only<B>(port: u16, request: Request<B>, next: Next<B>) -> Response {
    let host_ok = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| {
            host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
        });
    let json_ok = request.method() == axum::http::Method::GET
        || request
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value == HeaderValue::from_static("application/json"));
    if host_ok && json_ok {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

#[derive(Serialize)]
struct Presets {
    displays: BTreeMap<&'static str, Display>,
    patterns: BTreeMap<&'static str, Pattern>,
    environments: BTreeMap<&'static str, f64>,
}

#[derive(Serialize)]
pub(crate) struct StageReply {
    stage: Stage,
    derived: Derived,
}

#[derive(Serialize)]
struct StateReply {
    #[serde(flatten)]
    stage: StageReply,
    presets: Presets,
    hub: crate::hub::HubView,
}

pub(crate) fn stage_reply(compiled: &odyssey_stage::Compiled) -> Result<StageReply, ApiError> {
    Ok(StageReply {
        derived: App::derived(compiled)?,
        stage: compiled.stage().clone(),
    })
}

async fn state(State(app): Shared) -> Result<Json<StateReply>, ApiError> {
    let presets = Presets {
        displays: DISPLAY_PRESET_NAMES
            .iter()
            .filter_map(|name| display_preset(name).map(|display| (*name, display)))
            .collect(),
        patterns: PATTERN_NAMES
            .iter()
            .filter_map(|name| Pattern::named(name).map(|pattern| (*name, pattern)))
            .collect(),
        environments: ENVIRONMENTS.iter().copied().collect(),
    };
    Ok(Json(StateReply {
        stage: stage_reply(&app.compiled())?,
        presets,
        hub: app.hub.borrow().clone(),
    }))
}

async fn put_stage(
    State(app): Shared,
    Json(stage): Json<Stage>,
) -> Result<Json<StageReply>, ApiError> {
    let compiled = app.set_stage(&stage)?;
    Ok(Json(stage_reply(&compiled)?))
}

#[derive(Deserialize)]
struct RunRequest {
    playing: Option<bool>,
    speed: Option<f64>,
    looping: Option<bool>,
    t_s: Option<f64>,
}

async fn run(State(app): Shared, Json(request): Json<RunRequest>) -> StatusCode {
    app.clock()
        .set(request.playing, request.speed, request.looping, request.t_s);
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct PressRequest {
    device: String,
    button: Button,
}

async fn press(
    State(app): Shared,
    Json(request): Json<PressRequest>,
) -> Result<StatusCode, ApiError> {
    let compiled = app.compiled();
    let device = compiled.stage().device(&request.device)?;
    if device.tracker().is_none() {
        return Err(ApiError::BadRequest(format!(
            "{:?} is not a tracker",
            request.device
        )));
    }
    let bound = compiled
        .stage()
        .hub_devices()
        .find(|(_, bound)| bound.id == request.device);
    if let Some((unit, _)) = bound {
        let command = match request.button {
            Button::Trigger => Command::Shot,
            Button::Pair => Command::Pair { timeout_ms: 30_000 },
        };
        let mut simulator = app.sim.lock().await;
        let target = simulator.target(unit);
        simulator
            .execute(target, command)
            .await
            .map_err(ApiError::BadRequest)?;
    }
    let data =
        serde_json::json!({ "device": request.device, "button": request.button }).to_string();
    let _ = app.messages.send(Message {
        event: "press",
        data: data.into(),
    });
    Ok(StatusCode::NO_CONTENT)
}

/// A simulator command and the unit and dongle it is for. Without an address
/// it is for the first.
#[derive(Deserialize)]
struct SimRequest {
    #[serde(flatten)]
    target: Target,
    command: Command,
}

async fn sim_command(
    State(app): Shared,
    Json(request): Json<SimRequest>,
) -> Result<Json<odyssey_sim::Snapshot>, ApiError> {
    app.sim
        .lock()
        .await
        .execute(request.target, request.command)
        .await
        .map(Json)
        .map_err(ApiError::BadRequest)
}

/// Make the hub's first calibrated screen the staged display, so simulated
/// and physical devices share the display the hub's tracker believes in.
async fn adopt_hub_display(State(app): Shared) -> Result<Json<StageReply>, ApiError> {
    let screens = app
        .sim
        .lock()
        .await
        .hub_screens()
        .await
        .map_err(ApiError::BadRequest)?;
    let screen = screens
        .iter()
        .min_by_key(|screen| screen.id)
        .ok_or_else(|| ApiError::BadRequest("the hub has no calibrated screen".to_owned()))?;
    let mut stage = app.compiled().stage().clone();
    stage.display = Display::from_screen(screen, true)?;
    let compiled = app.set_stage(&stage)?;
    Ok(Json(stage_reply(&compiled)?))
}

#[derive(Deserialize)]
struct EvaluateRequest {
    device: String,
    fps: Option<f64>,
    tracker_knows_surface: Option<bool>,
}

async fn evaluate_run(
    State(app): Shared,
    Json(request): Json<EvaluateRequest>,
) -> Result<Json<Evaluation>, ApiError> {
    let compiled = app.compiled();
    Ok(Json(
        evaluate(
            &compiled,
            &request.device,
            request.fps.unwrap_or(30.0),
            request.tracker_knows_surface.unwrap_or(true),
        )
        .await?,
    ))
}

#[derive(Deserialize)]
struct ExportQuery {
    device: String,
    fps: Option<f64>,
}

async fn export_scene(
    State(app): Shared,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let scene = app
        .compiled()
        .scene(&query.device, query.fps.unwrap_or(30.0))?;
    let body =
        serde_json::to_vec_pretty(&scene).map_err(|error| ApiError::Internal(error.to_string()))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"staged-scene.json\""),
    );
    Ok((headers, body).into_response())
}

async fn events(State(app): Shared) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(app.messages.subscribe()).filter_map(|message| {
        message
            .ok()
            .map(|message| Ok(Event::default().event(message.event).data(&*message.data)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Method;
    use tokio::sync::watch;
    use tower::ServiceExt as _;

    use super::*;
    use crate::hub::HubView;

    async fn status(method: Method, host: &str, content_type: Option<&str>) -> StatusCode {
        let (_hub, view) = watch::channel(HubView::default());
        let app = App::new(&Stage::default(), view, None, None).unwrap();
        let mut request = Request::builder()
            .method(method)
            .uri("/api/run")
            .header(header::HOST, host);
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        let body = Body::from(r#"{"playing":true}"#);
        router(app, 7878)
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn requests_must_name_this_server_and_post_json() {
        let json = Some("application/json");
        assert_eq!(
            status(Method::POST, "127.0.0.1:7878", json).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            status(Method::POST, "localhost:7878", json).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            status(Method::POST, "evil.example:7878", json).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(Method::POST, "127.0.0.1:9", json).await,
            StatusCode::FORBIDDEN
        );
        let form = Some("application/x-www-form-urlencoded");
        assert_eq!(
            status(Method::POST, "127.0.0.1:7878", form).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(Method::POST, "127.0.0.1:7878", None).await,
            StatusCode::FORBIDDEN
        );
    }
}
