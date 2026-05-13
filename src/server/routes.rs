//! HTTP route handlers.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::StreamExt;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::core::{GetSessionConfig, Session, SessionMeta};

use crate::server::app::AppState;

#[derive(Debug, Deserialize)]
pub(crate) struct RunBody {
    agent: String,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    message: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentList {
    agents: Vec<String>,
}

pub(crate) async fn list_agents(State(state): State<AppState>) -> Json<AgentList> {
    Json(AgentList {
        agents: state.runners.keys().cloned().collect(),
    })
}

pub(crate) async fn run(
    State(state): State<AppState>,
    Json(body): Json<RunBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let runner = state.runners.get(&body.agent).cloned().ok_or((
        StatusCode::NOT_FOUND,
        format!("unknown agent: {}", body.agent),
    ))?;
    let user_id = body.user_id.as_deref().unwrap_or("anonymous");
    let session_id = body.session_id.as_deref();
    let mut stream = runner
        .run(user_id, session_id, &body.message)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(e) => events.push(e),
            Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        }
    }
    Ok(Json(
        serde_json::to_value(events).unwrap_or(serde_json::Value::Null),
    ))
}

pub(crate) async fn run_sse(
    State(state): State<AppState>,
    Json(body): Json<RunBody>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, (StatusCode, String)> {
    let runner = state.runners.get(&body.agent).cloned().ok_or((
        StatusCode::NOT_FOUND,
        format!("unknown agent: {}", body.agent),
    ))?;
    let user_id = body.user_id.unwrap_or_else(|| "anonymous".to_string());
    let session_id = body.session_id;
    let stream = async_stream::stream! {
        let r = runner
            .run(&user_id, session_id.as_deref(), &body.message)
            .await;
        match r {
            Ok(mut s) => {
                while let Some(ev) = s.next().await {
                    let payload = match ev {
                        Ok(e) => serde_json::to_string(&e).unwrap_or_else(|_| "{}".into()),
                        Err(e) => format!(r#"{{"error":"{}"}}"#, escape(&e.to_string())),
                    };
                    yield Ok(SseEvent::default().data(payload));
                }
                yield Ok::<_, Infallible>(SseEvent::default().event("done").data(""));
            }
            Err(e) => {
                error!("/run_sse setup failed: {e}");
                yield Ok(SseEvent::default()
                    .event("error")
                    .data(format!(r#"{{"error":"{}"}}"#, escape(&e.to_string()))));
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn escape(s: &str) -> String {
    s.replace('"', "\\\"")
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateSessionBody {
    #[serde(default)]
    pub session_id: Option<String>,
}

pub(crate) async fn create_session(
    State(state): State<AppState>,
    Path((app, user)): Path<(String, String)>,
    Json(body): Json<CreateSessionBody>,
) -> Result<Json<Session>, (StatusCode, String)> {
    // We don't know which runner serves this app; we use the first registered.
    let runner = first_runner(&state, &app)?;
    let s = runner
        .session_service()
        .create_session(&app, &user, None, body.session_id.as_deref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(s))
}

pub(crate) async fn list_sessions(
    State(state): State<AppState>,
    Path((app, user)): Path<(String, String)>,
) -> Result<Json<Vec<SessionMeta>>, (StatusCode, String)> {
    let runner = first_runner(&state, &app)?;
    let r = runner
        .session_service()
        .list_sessions(&app, &user)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(r.sessions))
}

pub(crate) async fn get_session(
    State(state): State<AppState>,
    Path((app, user, session)): Path<(String, String, String)>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let runner = first_runner(&state, &app)?;
    let r = runner
        .session_service()
        .get_session(&app, &user, &session, GetSessionConfig::default())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match r {
        Some(s) => Ok(Json(s).into_response()),
        None => Ok((
            StatusCode::NOT_FOUND,
            format!("session {session} not found"),
        )
            .into_response()),
    }
}

pub(crate) async fn delete_session(
    State(state): State<AppState>,
    Path((app, user, session)): Path<(String, String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let runner = first_runner(&state, &app)?;
    runner
        .session_service()
        .delete_session(&app, &user, &session)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

fn first_runner(
    state: &AppState,
    app: &str,
) -> Result<Arc<crate::runner::Runner>, (StatusCode, String)> {
    state
        .runners
        .values()
        .find(|r| r.app_name() == app)
        .cloned()
        .ok_or((
            StatusCode::NOT_FOUND,
            format!("no runner registered for app {app}"),
        ))
}
