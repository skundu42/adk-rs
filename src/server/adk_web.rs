//! `adk api_server`-compatible endpoint surface.
//!
//! Implements the wire contract of Python ADK's `adk api_server` / `adk web`
//! backend (FastAPI), so Google's adk-web Angular dev UI — and any other
//! `google-adk` API client — can talk to an adk-rs server: camelCase JSON,
//! `{"detail": ...}` errors, `data: <json>\n\n` SSE framing, and the
//! `/apps/{app}/users/{user}/sessions/...` path scheme.
//!
//! To use the dev UI, run adk-web pointed at this server (its
//! `runtime-config.json` `backendUrl`), and pass the UI's origin to
//! [`crate::server::AppState::with_allow_origins`] so CORS preflights pass.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{get, post};
use futures::StreamExt;
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::error;
use uuid::Uuid;

use crate::core::{Event, GetSessionConfig, RunConfig, StateDelta, StreamingMode};
use crate::genai_types::Content;
use crate::runner::Runner;

use crate::server::app::AppState;
use crate::server::wire::{event_from_wire, event_to_wire, session_meta_to_wire, session_to_wire};

/// FastAPI-style error response: `{"detail": "..."}` with a status code.
fn detail(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "detail": msg.into() })))
}

fn internal(e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    detail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Resolve the runner serving `app`.
fn runner_for(state: &AppState, app: &str) -> Result<Arc<Runner>, (StatusCode, Json<Value>)> {
    state
        .runners
        .values()
        .find(|r| r.app_name() == app)
        .cloned()
        .ok_or_else(|| detail(StatusCode::NOT_FOUND, format!("App not found: {app}")))
}

/// Routes implementing the `adk api_server` contract. Merged into the main
/// router by [`crate::server::build_router`].
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/list-apps", get(list_apps))
        .route("/run", post(run))
        .route("/run_sse", post(run_sse))
        .route(
            "/apps/:app/users/:user/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/apps/:app/users/:user/sessions/:session",
            get(get_session)
                .post(create_session_with_id)
                .delete(delete_session)
                .patch(patch_session),
        )
        .route(
            "/apps/:app/users/:user/sessions/:session/artifacts",
            get(list_artifacts),
        )
        .route(
            "/apps/:app/users/:user/sessions/:session/artifacts/:name",
            get(load_artifact).delete(delete_artifact),
        )
        .route(
            "/apps/:app/users/:user/sessions/:session/artifacts/:name/versions",
            get(list_artifact_versions),
        )
        .route(
            "/apps/:app/users/:user/sessions/:session/artifacts/:name/versions/:version",
            get(load_artifact_version),
        )
        .route("/apps/:app/users/:user/memory", axum::routing::patch(add_session_to_memory))
        // Trace endpoints (1.x paths + 2.x /dev/apps prefix). adk-rs does
        // not yet capture per-event spans server-side; these return empty
        // results so the UI's trace tab degrades gracefully.
        .route("/debug/trace/:event_id", get(trace_not_found))
        .route("/debug/trace/session/:session", get(empty_array))
        .route("/dev/apps/:app/debug/trace/:event_id", get(trace_not_found))
        .route(
            "/dev/apps/:app/debug/trace/session/:session",
            get(empty_array),
        )
        // Eval surface stubs (both path generations): adk-rs evals run via
        // the `adk eval` CLI today; the UI tab lists empty sets.
        .route("/apps/:app/eval_sets", get(empty_array))
        .route("/apps/:app/eval_results", get(empty_array))
        .route("/dev/apps/:app/eval_sets", get(empty_array))
        .route("/dev/apps/:app/eval_results", get(empty_array))
        .route("/dev/apps/:app/metrics-info", get(metrics_info))
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn version() -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "language": "rust",
        "language_version": option_env!("CARGO_PKG_RUST_VERSION").unwrap_or(""),
    }))
}

async fn list_apps(State(state): State<AppState>) -> Json<Vec<String>> {
    let mut apps: Vec<String> = state
        .runners
        .values()
        .map(|r| r.app_name().to_string())
        .collect();
    apps.sort();
    apps.dedup();
    Json(apps)
}

async fn trace_not_found() -> (StatusCode, Json<Value>) {
    detail(StatusCode::NOT_FOUND, "Trace not found")
}

async fn empty_array() -> Json<Value> {
    Json(json!([]))
}

async fn metrics_info() -> Json<Value> {
    Json(json!({ "metricsInfo": [] }))
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// Body of `POST .../sessions`. Accepts camelCase aliases (the wire default)
/// and snake_case (pydantic `populate_by_name`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionRequest {
    #[serde(alias = "session_id")]
    session_id: Option<String>,
    state: Option<StateDelta>,
    events: Option<Vec<Value>>,
}

async fn create_session(
    State(state): State<AppState>,
    Path((app, user)): Path<(String, String)>,
    body: Option<Json<Option<CreateSessionRequest>>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let req = body.and_then(|Json(b)| b).unwrap_or_default();
    create_session_impl(&state, &app, &user, req).await
}

async fn create_session_with_id(
    State(state): State<AppState>,
    Path((app, user, session)): Path<(String, String, String)>,
    body: Option<Json<Option<StateDelta>>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let req = CreateSessionRequest {
        session_id: Some(session),
        state: body.and_then(|Json(b)| b),
        events: None,
    };
    create_session_impl(&state, &app, &user, req).await
}

async fn create_session_impl(
    state: &AppState,
    app: &str,
    user: &str,
    req: CreateSessionRequest,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(state, app)?;
    let svc = runner.session_service();
    if let Some(sid) = req.session_id.as_deref() {
        let existing = svc
            .get_session(app, user, sid, GetSessionConfig::default())
            .await
            .map_err(internal)?;
        if existing.is_some() {
            return Err(detail(
                StatusCode::CONFLICT,
                format!("Session already exists: {sid}"),
            ));
        }
    }
    let initial_state = req.state.map(crate::core::State::from_iter);
    let mut session = svc
        .create_session(app, user, initial_state, req.session_id.as_deref())
        .await
        .map_err(internal)?;
    for wire_event in req.events.unwrap_or_default() {
        let Some(ev) = event_from_wire(&wire_event) else {
            continue;
        };
        svc.append_event(&mut session, ev).await.map_err(internal)?;
    }
    Ok(Json(session_to_wire(&session)))
}

async fn list_sessions(
    State(state): State<AppState>,
    Path((app, user)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let r = runner
        .session_service()
        .list_sessions(&app, &user)
        .await
        .map_err(internal)?;
    Ok(Json(Value::Array(
        r.sessions.iter().map(session_meta_to_wire).collect(),
    )))
}

async fn get_session(
    State(state): State<AppState>,
    Path((app, user, session)): Path<(String, String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let s = runner
        .session_service()
        .get_session(&app, &user, &session, GetSessionConfig::default())
        .await
        .map_err(internal)?
        .ok_or_else(|| detail(StatusCode::NOT_FOUND, "Session not found"))?;
    Ok(Json(session_to_wire(&s)))
}

async fn delete_session(
    State(state): State<AppState>,
    Path((app, user, session)): Path<(String, String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    runner
        .session_service()
        .delete_session(&app, &user, &session)
        .await
        .map_err(internal)?;
    // Python returns a 200 with a `null` body.
    Ok(Json(Value::Null))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSessionRequest {
    #[serde(alias = "state_delta", default)]
    state_delta: StateDelta,
}

async fn patch_session(
    State(state): State<AppState>,
    Path((app, user, session)): Path<(String, String, String)>,
    Json(body): Json<UpdateSessionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let svc = runner.session_service();
    let mut s = svc
        .get_session(&app, &user, &session, GetSessionConfig::default())
        .await
        .map_err(internal)?
        .ok_or_else(|| detail(StatusCode::NOT_FOUND, "Session not found"))?;
    // Mirror Python: a synthetic user event carrying the state delta.
    let mut ev = Event::new("user", crate::core::LlmResponse::default());
    ev.invocation_id = format!("p-{}", Uuid::new_v4());
    ev.actions.state_delta = body.state_delta;
    svc.append_event(&mut s, ev).await.map_err(internal)?;
    Ok(Json(session_to_wire(&s)))
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

/// Body of `POST /run` and `POST /run_sse` (Python `RunAgentRequest`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRunRequest {
    #[serde(alias = "app_name")]
    app_name: Option<String>,
    #[serde(alias = "user_id")]
    user_id: String,
    #[serde(alias = "session_id")]
    session_id: String,
    #[serde(alias = "new_message")]
    new_message: Option<Content>,
    #[serde(default)]
    streaming: bool,
    #[serde(alias = "state_delta", default)]
    state_delta: Option<StateDelta>,
    #[serde(alias = "invocation_id")]
    invocation_id: Option<String>,
    // Accepted for wire compatibility; resume matching is driven by the
    // function-call ids inside `new_message`.
    #[serde(alias = "function_call_event_id")]
    #[allow(dead_code)]
    function_call_event_id: Option<String>,
}

/// Resolve the runner for a run request; with a single registered app the
/// `appName` field may be omitted (mirrors Python's default-app fallback).
fn run_runner(
    state: &AppState,
    req: &AgentRunRequest,
) -> Result<Arc<Runner>, (StatusCode, Json<Value>)> {
    match &req.app_name {
        Some(app) => runner_for(state, app),
        None => {
            let mut apps: Vec<&Arc<Runner>> = state.runners.values().collect();
            apps.dedup_by(|a, b| a.app_name() == b.app_name());
            if apps.len() == 1 {
                Ok(apps[0].clone())
            } else {
                Err(detail(
                    StatusCode::BAD_REQUEST,
                    "appName is required when multiple apps are registered",
                ))
            }
        }
    }
}

/// Common pre-run work: verify the session exists, apply any `stateDelta`,
/// and start (or resume) the invocation.
async fn start_run(
    runner: &Arc<Runner>,
    req: &AgentRunRequest,
    streaming: bool,
) -> Result<crate::runner::RunningInvocation, (StatusCode, Json<Value>)> {
    let app = runner.app_name().to_string();
    let svc = runner.session_service();
    let mut session = svc
        .get_session(&app, &req.user_id, &req.session_id, GetSessionConfig::default())
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            detail(
                StatusCode::NOT_FOUND,
                format!("Session not found: {}", req.session_id),
            )
        })?;

    if let Some(delta) = req.state_delta.as_ref().filter(|d| !d.is_empty()) {
        let mut ev = Event::new("user", crate::core::LlmResponse::default());
        ev.invocation_id = format!("p-{}", Uuid::new_v4());
        ev.actions.state_delta = delta.clone();
        svc.append_event(&mut session, ev).await.map_err(internal)?;
    }

    let run_config = RunConfig {
        streaming_mode: if streaming {
            StreamingMode::Sse
        } else {
            StreamingMode::None
        },
        ..RunConfig::default()
    };
    let handle = match &req.invocation_id {
        Some(inv) => {
            runner
                .resume(
                    &req.user_id,
                    &req.session_id,
                    inv,
                    req.new_message.clone(),
                    run_config,
                )
                .await
        }
        None => {
            let content = req
                .new_message
                .clone()
                .ok_or_else(|| detail(StatusCode::BAD_REQUEST, "newMessage is required"))?;
            runner
                .start(&req.user_id, Some(&req.session_id), content, run_config)
                .await
        }
    }
    .map_err(internal)?;
    Ok(handle)
}

async fn run(
    State(state): State<AppState>,
    Json(req): Json<AgentRunRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = run_runner(&state, &req)?;
    let handle = start_run(&runner, &req, false).await?;
    let mut events = Vec::new();
    let mut stream = handle.events;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(e) => events.push(event_to_wire(&e)),
            Err(e) => return Err(internal(e)),
        }
    }
    Ok(Json(Value::Array(events)))
}

async fn run_sse(
    State(state): State<AppState>,
    Json(req): Json<AgentRunRequest>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, (StatusCode, Json<Value>)> {
    let runner = run_runner(&state, &req)?;
    let streaming = req.streaming;
    // Session existence (and stateDelta) is validated before streaming
    // starts, so missing sessions are normal HTTP 404s — like Python.
    let handle = start_run(&runner, &req, streaming).await?;
    let stream = async_stream::stream! {
        let mut s = handle.events;
        while let Some(ev) = s.next().await {
            match ev {
                Ok(e) => {
                    for frame in sse_frames_for_event(&e) {
                        yield Ok(SseEvent::default().data(frame));
                    }
                }
                Err(e) => {
                    error!("/run_sse: {e}");
                    yield Ok::<_, Infallible>(SseEvent::default().data(
                        json!({ "error": e.to_string() }).to_string(),
                    ));
                    return;
                }
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Frame one event for `/run_sse`. Mirrors Python's artifact-splitting
/// quirk: an event carrying both content parts and a non-empty
/// `artifactDelta` is emitted as two frames — content first (delta cleared),
/// then the delta without content — so the UI renders both reliably.
fn sse_frames_for_event(e: &Event) -> Vec<String> {
    let has_parts = e
        .response
        .content
        .as_ref()
        .is_some_and(|c| !c.parts.is_empty());
    if has_parts && !e.actions.artifact_delta.is_empty() {
        let mut content_only = e.clone();
        content_only.actions.artifact_delta = Default::default();
        let mut delta_only = e.clone();
        delta_only.response.content = None;
        vec![
            event_to_wire(&content_only).to_string(),
            event_to_wire(&delta_only).to_string(),
        ]
    } else {
        vec![event_to_wire(e).to_string()]
    }
}

// ---------------------------------------------------------------------------
// Artifacts
// ---------------------------------------------------------------------------

fn artifact_service(
    runner: &Arc<Runner>,
) -> Result<Arc<dyn crate::core::ArtifactService>, (StatusCode, Json<Value>)> {
    runner.artifact_service().cloned().ok_or_else(|| {
        detail(
            StatusCode::NOT_FOUND,
            "Artifact service is not configured",
        )
    })
}

fn artifact_key(app: &str, user: &str, session: &str, name: &str) -> crate::core::ArtifactKey {
    crate::core::ArtifactKey::new(app, user, session, name)
}

async fn list_artifacts(
    State(state): State<AppState>,
    Path((app, user, session)): Path<(String, String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let svc = artifact_service(&runner)?;
    let names = svc
        .list_artifact_keys(&app, &user, &session)
        .await
        .map_err(internal)?;
    Ok(Json(json!(names)))
}

#[derive(Debug, Deserialize)]
struct VersionQuery {
    version: Option<u64>,
}

async fn load_artifact(
    State(state): State<AppState>,
    Path((app, user, session, name)): Path<(String, String, String, String)>,
    Query(q): Query<VersionQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let svc = artifact_service(&runner)?;
    let part = svc
        .load_artifact(artifact_key(&app, &user, &session, &name), q.version)
        .await
        .map_err(internal)?
        .ok_or_else(|| detail(StatusCode::NOT_FOUND, "Artifact not found"))?;
    Ok(Json(serde_json::to_value(&part).map_err(internal)?))
}

async fn load_artifact_version(
    State(state): State<AppState>,
    Path((app, user, session, name, version)): Path<(String, String, String, String, u64)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let svc = artifact_service(&runner)?;
    let part = svc
        .load_artifact(artifact_key(&app, &user, &session, &name), Some(version))
        .await
        .map_err(internal)?
        .ok_or_else(|| detail(StatusCode::NOT_FOUND, "Artifact not found"))?;
    Ok(Json(serde_json::to_value(&part).map_err(internal)?))
}

async fn list_artifact_versions(
    State(state): State<AppState>,
    Path((app, user, session, name)): Path<(String, String, String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let svc = artifact_service(&runner)?;
    let versions = svc
        .list_versions(artifact_key(&app, &user, &session, &name))
        .await
        .map_err(internal)?;
    Ok(Json(json!(versions)))
}

async fn delete_artifact(
    State(state): State<AppState>,
    Path((app, user, session, name)): Path<(String, String, String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let svc = artifact_service(&runner)?;
    svc.delete_artifact(artifact_key(&app, &user, &session, &name))
        .await
        .map_err(internal)?;
    Ok(Json(Value::Null))
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMemoryRequest {
    #[serde(alias = "session_id")]
    session_id: String,
}

async fn add_session_to_memory(
    State(state): State<AppState>,
    Path((app, user)): Path<(String, String)>,
    Json(body): Json<UpdateMemoryRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let runner = runner_for(&state, &app)?;
    let memory = runner
        .memory_service()
        .cloned()
        .ok_or_else(|| detail(StatusCode::BAD_REQUEST, "Memory service is not configured"))?;
    let session = runner
        .session_service()
        .get_session(&app, &user, &body.session_id, GetSessionConfig::default())
        .await
        .map_err(internal)?
        .ok_or_else(|| detail(StatusCode::NOT_FOUND, "Session not found"))?;
    memory
        .add_session_to_memory(&session)
        .await
        .map_err(internal)?;
    Ok(Json(Value::Null))
}
