//! A2A server bridge — exposes a [`crate::runner::Runner`] over the A2A
//! JSON-RPC protocol.
//!
//! Routes mounted by [`router`]:
//!
//! - `GET  /.well-known/agent.json` (default path; configurable via
//!   [`A2aServerConfig::agent_card_path`]) — returns the [`AgentCard`].
//! - `POST <rpc_path>` (default `/`) — JSON-RPC endpoint dispatching:
//!     - [`method::MESSAGE_SEND`] — synchronous: create a task, run the
//!       agent to completion, return the final [`Task`].
//!     - [`method::MESSAGE_STREAM`] — SSE: same as above but streams every
//!       intermediate [`TaskStatusUpdateEvent`] / [`TaskArtifactUpdateEvent`]
//!       to the caller.
//!     - [`method::TASKS_GET`] — return a task by id.
//!     - [`method::TASKS_CANCEL`] — cancel a non-terminal task.
//!     - [`method::TASKS_RESUBSCRIBE`] — re-attach to a task's SSE channel.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt;
use futures::stream::Stream;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::core::{Event, RunConfig};
use crate::error::Result;
use crate::genai_types::Content;
use crate::runner::Runner;

use super::mapping::{a2a_part_to_adk, artifact_chunk, event_to_message, new_task};
use super::push_notifier::PushNotifier;
use super::task_service::{InMemoryTaskService, TaskService, TaskUpdate, rfc3339_now};
use super::types::{
    A2aError, A2aRequest, A2aResponse, AgentCard, Artifact, GetTaskPushNotificationConfigParams,
    ListTaskPushNotificationConfigResult, Message, MessageRole, MessageSendParams, Part,
    StreamingMessageResult, Task, TaskIdParams, TaskPushNotificationConfig, TaskQueryParams,
    TaskState, TaskStatus, method,
};

/// Server bootstrap config: agent card + paths + auth.
#[derive(Debug, Clone)]
pub struct A2aServerConfig {
    /// The [`AgentCard`] returned at [`A2aServerConfig::agent_card_path`].
    pub agent_card: AgentCard,
    /// Path the agent card is served at. Defaults to
    /// `/.well-known/agent.json` per the A2A spec.
    pub agent_card_path: String,
    /// Path the JSON-RPC endpoint is mounted at. Defaults to `/`.
    pub rpc_path: String,
    /// Optional bearer token required on every JSON-RPC request.
    pub auth_token: Option<Arc<String>>,
}

impl A2aServerConfig {
    /// Helper: build a config that advertises an agent card with streaming
    /// enabled, exposed under the default paths.
    pub fn new(agent_card: AgentCard) -> Self {
        Self {
            agent_card,
            agent_card_path: "/.well-known/agent.json".into(),
            rpc_path: "/".into(),
            auth_token: None,
        }
    }

    /// Require an `Authorization: Bearer <token>` header on every JSON-RPC
    /// request.
    #[must_use]
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(Arc::new(token.into()));
        self
    }
}

/// Shared axum state.
#[derive(Clone)]
pub struct A2aState {
    /// The runner that handles incoming requests.
    pub runner: Arc<Runner>,
    /// Persistence for [`Task`] state.
    pub tasks: Arc<dyn TaskService>,
    /// Outbound webhook dispatcher for `tasks/pushNotificationConfig/*`.
    /// Created once per state; spawns per-subscriber background tasks
    /// internally.
    pub push: Arc<PushNotifier>,
    /// Agent card served by the discovery endpoint.
    pub agent_card: Arc<AgentCard>,
    /// Agent card mount path.
    pub agent_card_path: Arc<String>,
    /// JSON-RPC mount path (echoed into the agent card warnings if
    /// applicable).
    pub rpc_path: Arc<String>,
    /// Optional bearer token.
    pub auth_token: Option<Arc<String>>,
}

impl A2aState {
    /// Build from a runner + a config, using [`InMemoryTaskService`] for
    /// task persistence.
    pub fn new(runner: Arc<Runner>, cfg: A2aServerConfig) -> Self {
        let tasks: Arc<dyn TaskService> = Arc::new(InMemoryTaskService::new());
        Self::assemble(runner, cfg, tasks)
    }

    /// Build with an explicit [`TaskService`].
    pub fn with_task_service(
        runner: Arc<Runner>,
        cfg: A2aServerConfig,
        tasks: Arc<dyn TaskService>,
    ) -> Self {
        Self::assemble(runner, cfg, tasks)
    }

    fn assemble(runner: Arc<Runner>, cfg: A2aServerConfig, tasks: Arc<dyn TaskService>) -> Self {
        let push = Arc::new(
            PushNotifier::new(tasks.clone())
                .expect("PushNotifier requires a working reqwest HTTP client"),
        );
        // If the card advertises streaming, it also advertises push by
        // default — but the source of truth is the agent_card field itself,
        // which the caller filled in. We don't mutate it here.
        let mut card = cfg.agent_card;
        // Make sure the advertised capability matches reality: this server
        // supports push notifications.
        card.capabilities.push_notifications = true;
        Self {
            runner,
            tasks,
            push,
            agent_card: Arc::new(card),
            agent_card_path: Arc::new(cfg.agent_card_path),
            rpc_path: Arc::new(cfg.rpc_path),
            auth_token: cfg.auth_token,
        }
    }
}

impl std::fmt::Debug for A2aState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A2aState")
            .field("app_name", &self.runner.app_name())
            .field("agent", &self.agent_card.name)
            .field("auth_token", &self.auth_token.as_ref().map(|_| "<set>"))
            .finish()
    }
}

/// Options for [`serve_with`].
#[derive(Debug, Clone, Default)]
pub struct ServeOptions {
    /// Allow non-loopback binds without an auth token.
    pub dangerously_allow_unauthenticated_remote: bool,
}

/// Build an [`axum::Router`] mounting the JSON-RPC endpoint and the agent
/// card endpoint.
pub fn router(state: A2aState) -> Router {
    let rpc_path = state.rpc_path.clone();
    let agent_card_path = state.agent_card_path.clone();
    let rpc = post(rpc_handler);
    let inner = Router::new()
        .route(agent_card_path.as_str(), get(agent_card_handler))
        .route(rpc_path.as_str(), rpc);
    if state.auth_token.is_some() {
        let token_state = state.clone();
        inner
            .route_layer(middleware::from_fn_with_state(token_state, require_bearer))
            .with_state(state)
    } else {
        inner.with_state(state)
    }
}

/// Bind and serve. Refuses non-loopback binds when no auth token is set
/// (override via [`serve_with`] + [`ServeOptions`]).
pub async fn serve(addr: SocketAddr, state: A2aState) -> Result<()> {
    serve_with(addr, state, ServeOptions::default()).await
}

/// Like [`serve`], but accepts explicit [`ServeOptions`].
pub async fn serve_with(addr: SocketAddr, state: A2aState, opts: ServeOptions) -> Result<()> {
    if !addr.ip().is_loopback() {
        let has_auth = state.auth_token.is_some();
        if !has_auth && !opts.dangerously_allow_unauthenticated_remote {
            return Err(crate::error::Error::config(format!(
                "refusing to bind A2A server on non-loopback address {addr} without auth — \
                 set A2aServerConfig::with_bearer_token(...) or pass \
                 ServeOptions::dangerously_allow_unauthenticated_remote=true to opt out"
            )));
        }
        warn!(
            "a2a-server bound on non-loopback {addr}: every remote caller can drive the agent{} — proceed only if this is what you intended",
            if has_auth {
                " (bearer token required)"
            } else {
                " AND NO AUTHENTICATION IS ENFORCED"
            }
        );
    }
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| crate::error::Error::other(format!("bind {addr}: {e}")))?;
    info!("a2a-server listening on http://{addr}");
    axum::serve(listener, app)
        .await
        .map_err(|e| crate::error::Error::other(format!("serve: {e}")))
}

async fn require_bearer(
    State(state): State<A2aState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(expected) = state.auth_token.as_ref() else {
        return next.run(req).await;
    };
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.strip_prefix("Bearer ")
                .or_else(|| s.strip_prefix("bearer "))
        });
    let ok = presented
        .map(|tok| constant_time_eq(expected.as_bytes(), tok.as_bytes()))
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        let mut resp = (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"adk-rs-a2a\""),
        );
        resp
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn agent_card_handler(State(state): State<A2aState>) -> Json<AgentCard> {
    Json((*state.agent_card).clone())
}

async fn rpc_handler(State(state): State<A2aState>, Json(req): Json<A2aRequest>) -> Response {
    let id = req.id.clone();
    if req.jsonrpc != "2.0" {
        return Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::INVALID_REQUEST, "jsonrpc must be \"2.0\""),
        ))
        .into_response();
    }
    match req.method.as_str() {
        method::MESSAGE_SEND => handle_message_send(state, id, req.params).await,
        method::MESSAGE_STREAM => handle_message_stream(state, id, req.params).into_response(),
        method::TASKS_GET => handle_tasks_get(state, id, req.params).await,
        method::TASKS_CANCEL => handle_tasks_cancel(state, id, req.params).await,
        method::TASKS_RESUBSCRIBE => {
            handle_tasks_resubscribe(state, id, req.params).into_response()
        }
        method::TASKS_PUSH_NOTIFICATION_CONFIG_SET => {
            handle_push_config_set(state, id, req.params).await
        }
        method::TASKS_PUSH_NOTIFICATION_CONFIG_GET => {
            handle_push_config_get(state, id, req.params).await
        }
        method::TASKS_PUSH_NOTIFICATION_CONFIG_LIST => {
            handle_push_config_list(state, id, req.params).await
        }
        method::TASKS_PUSH_NOTIFICATION_CONFIG_DELETE => {
            handle_push_config_delete(state, id, req.params).await
        }
        other => Json(A2aResponse::err(
            id,
            A2aError::new(
                A2aError::METHOD_NOT_FOUND,
                format!("unknown method: {other}"),
            ),
        ))
        .into_response(),
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    params: &Option<Value>,
) -> std::result::Result<T, A2aError> {
    let v = params
        .as_ref()
        .ok_or_else(|| A2aError::new(A2aError::INVALID_PARAMS, "missing params"))?;
    serde_json::from_value::<T>(v.clone())
        .map_err(|e| A2aError::new(A2aError::INVALID_PARAMS, e.to_string()))
}

async fn handle_message_send(
    state: A2aState,
    id: Option<Value>,
    params: Option<Value>,
) -> Response {
    let params: MessageSendParams = match decode(&params) {
        Ok(p) => p,
        Err(e) => return Json(A2aResponse::err(id, e)).into_response(),
    };
    let (task, mut events) = match start_task(&state, &params).await {
        Ok(v) => v,
        Err(e) => {
            error!("a2a message/send: {e}");
            return Json(A2aResponse::err(
                id,
                A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
            ))
            .into_response();
        }
    };
    let task_id = task.id.clone();
    let context_id = task.context_id.clone();

    // Drain the event stream synchronously, accumulating artifacts.
    let agg = Aggregator::new(task_id.clone(), context_id.clone());
    let agg = Arc::new(Mutex::new(agg));
    while let Some(ev) = events.next().await {
        match ev {
            Ok(ev) => {
                let mut a = agg.lock();
                a.absorb(&ev);
            }
            Err(e) => {
                let _ = state
                    .tasks
                    .update_status(
                        &task_id,
                        TaskStatus {
                            state: TaskState::Failed,
                            message: Some(Message::agent_text(e.to_string())),
                            timestamp: Some(rfc3339_now()),
                        },
                        true,
                    )
                    .await;
                return Json(A2aResponse::err(
                    id,
                    A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
                ))
                .into_response();
            }
        }
    }
    // Apply queued mutations now that we're outside the lock again.
    let pending = agg.lock().drain_pending();
    for p in pending {
        p.apply(&state).await;
    }
    let _ = state
        .tasks
        .update_status(
            &task_id,
            TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: Some(rfc3339_now()),
            },
            true,
        )
        .await;
    match state.tasks.get_task(&task_id, None).await {
        Ok(Some(final_task)) => Json(A2aResponse::ok(
            id,
            serde_json::to_value(&final_task).unwrap(),
        ))
        .into_response(),
        Ok(None) => Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::TASK_NOT_FOUND, format!("task {task_id} vanished")),
        ))
        .into_response(),
        Err(e) => Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
        ))
        .into_response(),
    }
}

fn handle_message_stream(
    state: A2aState,
    id: Option<Value>,
    params: Option<Value>,
) -> Sse<impl Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let stream = async_stream::stream! {
        let params: MessageSendParams = match decode(&params) {
            Ok(p) => p,
            Err(e) => {
                yield Ok(sse_frame(&A2aResponse::err(id.clone(), e)));
                return;
            }
        };
        let (task, mut events) = match start_task(&state, &params).await {
            Ok(v) => v,
            Err(e) => {
                error!("a2a message/stream: {e}");
                yield Ok(sse_frame(&A2aResponse::err(
                    id.clone(),
                    A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
                )));
                return;
            }
        };
        let task_id = task.id.clone();
        let context_id = task.context_id.clone();
        // Emit the initial Submitted task so the caller has the id.
        yield Ok(sse_result(&id, StreamingMessageResult::Task(task)));

        // Subscribe to live updates BEFORE we start running the agent so we
        // don't miss the Working/Completed transitions.
        let mut sub = match state.tasks.subscribe(&task_id).await {
            Ok(Some(rx)) => rx,
            Ok(None) => {
                yield Ok(sse_frame(&A2aResponse::err(
                    id.clone(),
                    A2aError::new(A2aError::INTERNAL_ERROR, "task not subscribable"),
                )));
                return;
            }
            Err(e) => {
                yield Ok(sse_frame(&A2aResponse::err(
                    id.clone(),
                    A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
                )));
                return;
            }
        };

        // Spawn the agent runner so we can interleave its event stream with
        // the broadcast channel.
        let svc = state.clone();
        let task_id_for_run = task_id.clone();
        let context_id_for_run = context_id.clone();
        let runner_task = tokio::spawn(async move {
            let mut agg = Aggregator::new(task_id_for_run.clone(), context_id_for_run);
            let mut last_err: Option<String> = None;
            while let Some(ev) = events.next().await {
                match ev {
                    Ok(ev) => agg.absorb(&ev),
                    Err(e) => {
                        last_err = Some(e.to_string());
                        break;
                    }
                }
            }
            for p in agg.drain_pending() {
                p.apply(&svc).await;
            }
            if let Some(e) = last_err {
                let _ = svc.tasks
                    .update_status(
                        &task_id_for_run,
                        TaskStatus {
                            state: TaskState::Failed,
                            message: Some(Message::agent_text(e)),
                            timestamp: Some(rfc3339_now()),
                        },
                        true,
                    )
                    .await;
            } else {
                let _ = svc.tasks
                    .update_status(
                        &task_id_for_run,
                        TaskStatus {
                            state: TaskState::Completed,
                            message: None,
                            timestamp: Some(rfc3339_now()),
                        },
                        true,
                    )
                    .await;
            }
        });

        loop {
            match sub.recv().await {
                Ok(TaskUpdate::Status(s)) => {
                    let is_final = s.is_final;
                    yield Ok(sse_result(&id, StreamingMessageResult::Status(s)));
                    if is_final {
                        break;
                    }
                }
                Ok(TaskUpdate::Artifact(a)) => {
                    yield Ok(sse_result(&id, StreamingMessageResult::Artifact(a)));
                }
                Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(_)) => continue,
            }
        }
        // Ensure the runner has fully completed before we close. This
        // bounds the stream's lifetime to the agent's, not the broadcast
        // channel's.
        let _ = runner_task.await;
        // Final SSE frame: a `done` event so clients can cleanly close.
        yield Ok(SseEvent::default().event("done").data(""));
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn handle_tasks_get(state: A2aState, id: Option<Value>, params: Option<Value>) -> Response {
    let params: TaskQueryParams = match decode(&params) {
        Ok(p) => p,
        Err(e) => return Json(A2aResponse::err(id, e)).into_response(),
    };
    match state
        .tasks
        .get_task(&params.id, params.history_length)
        .await
    {
        Ok(Some(t)) => Json(A2aResponse::ok(id, serde_json::to_value(&t).unwrap())).into_response(),
        Ok(None) => Json(A2aResponse::err(
            id,
            A2aError::new(
                A2aError::TASK_NOT_FOUND,
                format!("task {} not found", params.id),
            ),
        ))
        .into_response(),
        Err(e) => Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
        ))
        .into_response(),
    }
}

async fn handle_tasks_cancel(
    state: A2aState,
    id: Option<Value>,
    params: Option<Value>,
) -> Response {
    let params: TaskIdParams = match decode(&params) {
        Ok(p) => p,
        Err(e) => return Json(A2aResponse::err(id, e)).into_response(),
    };
    // First, flip the cancellation token on the underlying runner
    // invocation so the agent stops as soon as it observes it. We look the
    // id up via the task's metadata (`adk:invocationId`, set by
    // `start_task`). This is best-effort: if the task wasn't created from
    // a runner invocation (e.g. it was synthesised by an external caller
    // for testing), there's nothing to cancel.
    if let Ok(Some(t)) = state.tasks.get_task(&params.id, None).await {
        if let Some(inv_id) = t
            .metadata
            .as_ref()
            .and_then(|m| m.get("adk:invocationId"))
            .and_then(|v| v.as_str())
        {
            state.runner.cancel(inv_id);
        }
    }
    match state.tasks.cancel_task(&params.id).await {
        Ok(Some(t)) => Json(A2aResponse::ok(id, serde_json::to_value(&t).unwrap())).into_response(),
        Ok(None) => Json(A2aResponse::err(
            id,
            A2aError::new(
                A2aError::TASK_NOT_FOUND,
                format!("task {} not found", params.id),
            ),
        ))
        .into_response(),
        Err(e) => Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::TASK_NOT_CANCELABLE, e.to_string()),
        ))
        .into_response(),
    }
}

fn handle_tasks_resubscribe(
    state: A2aState,
    id: Option<Value>,
    params: Option<Value>,
) -> Sse<impl Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let stream = async_stream::stream! {
        let params: TaskIdParams = match decode(&params) {
            Ok(p) => p,
            Err(e) => {
                yield Ok(sse_frame(&A2aResponse::err(id.clone(), e)));
                return;
            }
        };
        match state.tasks.get_task(&params.id, None).await {
            Ok(Some(t)) => {
                yield Ok(sse_result(&id, StreamingMessageResult::Task(t.clone())));
                if t.status.state.is_terminal() {
                    yield Ok(SseEvent::default().event("done").data(""));
                    return;
                }
            }
            Ok(None) => {
                yield Ok(sse_frame(&A2aResponse::err(
                    id.clone(),
                    A2aError::new(
                        A2aError::TASK_NOT_FOUND,
                        format!("task {} not found", params.id),
                    ),
                )));
                return;
            }
            Err(e) => {
                yield Ok(sse_frame(&A2aResponse::err(
                    id.clone(),
                    A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
                )));
                return;
            }
        }
        let mut sub = match state.tasks.subscribe(&params.id).await {
            Ok(Some(rx)) => rx,
            // The task already terminated between get_task and subscribe.
            Ok(None) => {
                yield Ok(SseEvent::default().event("done").data(""));
                return;
            }
            Err(e) => {
                yield Ok(sse_frame(&A2aResponse::err(
                    id.clone(),
                    A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
                )));
                return;
            }
        };
        loop {
            match sub.recv().await {
                Ok(TaskUpdate::Status(s)) => {
                    let is_final = s.is_final;
                    yield Ok(sse_result(&id, StreamingMessageResult::Status(s)));
                    if is_final { break; }
                }
                Ok(TaskUpdate::Artifact(a)) => {
                    yield Ok(sse_result(&id, StreamingMessageResult::Artifact(a)));
                }
                Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(_)) => continue,
            }
        }
        yield Ok(SseEvent::default().event("done").data(""));
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn handle_push_config_set(
    state: A2aState,
    id: Option<Value>,
    params: Option<Value>,
) -> Response {
    let bundle: TaskPushNotificationConfig = match decode(&params) {
        Ok(p) => p,
        Err(e) => return Json(A2aResponse::err(id, e)).into_response(),
    };
    // Refuse a config that would leak credentials over plaintext HTTP.
    if let Err(e) = crate::transport_security::require_secure_url(
        &bundle.push_notification_config.url,
        "PushNotificationConfig.url",
    ) {
        return Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::INVALID_PARAMS, e.to_string()),
        ))
        .into_response();
    }
    match state
        .tasks
        .set_push_config(&bundle.task_id, bundle.push_notification_config)
        .await
    {
        Ok(Some(stored)) => {
            // Start the webhook subscriber for this config.
            state.push.register(&bundle.task_id, &stored).await;
            let result = TaskPushNotificationConfig {
                task_id: bundle.task_id,
                push_notification_config: stored,
            };
            Json(A2aResponse::ok(id, serde_json::to_value(&result).unwrap())).into_response()
        }
        Ok(None) => Json(A2aResponse::err(
            id,
            A2aError::new(
                A2aError::TASK_NOT_FOUND,
                format!("task {} not found", bundle.task_id),
            ),
        ))
        .into_response(),
        Err(e) => Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
        ))
        .into_response(),
    }
}

async fn handle_push_config_get(
    state: A2aState,
    id: Option<Value>,
    params: Option<Value>,
) -> Response {
    let params: GetTaskPushNotificationConfigParams = match decode(&params) {
        Ok(p) => p,
        Err(e) => return Json(A2aResponse::err(id, e)).into_response(),
    };
    match state
        .tasks
        .get_push_config(&params.id, params.push_notification_config_id.as_deref())
        .await
    {
        Ok(Some(cfg)) => {
            let bundle = TaskPushNotificationConfig {
                task_id: params.id,
                push_notification_config: cfg,
            };
            Json(A2aResponse::ok(id, serde_json::to_value(&bundle).unwrap())).into_response()
        }
        Ok(None) => Json(A2aResponse::err(
            id,
            A2aError::new(
                A2aError::TASK_NOT_FOUND,
                format!("no push config on task {}", params.id),
            ),
        ))
        .into_response(),
        Err(e) => Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
        ))
        .into_response(),
    }
}

async fn handle_push_config_list(
    state: A2aState,
    id: Option<Value>,
    params: Option<Value>,
) -> Response {
    let params: TaskIdParams = match decode(&params) {
        Ok(p) => p,
        Err(e) => return Json(A2aResponse::err(id, e)).into_response(),
    };
    match state.tasks.list_push_configs(&params.id).await {
        Ok(cfgs) => {
            let result: ListTaskPushNotificationConfigResult = cfgs
                .into_iter()
                .map(|c| TaskPushNotificationConfig {
                    task_id: params.id.clone(),
                    push_notification_config: c,
                })
                .collect();
            Json(A2aResponse::ok(id, serde_json::to_value(&result).unwrap())).into_response()
        }
        Err(e) => Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
        ))
        .into_response(),
    }
}

async fn handle_push_config_delete(
    state: A2aState,
    id: Option<Value>,
    params: Option<Value>,
) -> Response {
    let params: GetTaskPushNotificationConfigParams = match decode(&params) {
        Ok(p) => p,
        Err(e) => return Json(A2aResponse::err(id, e)).into_response(),
    };
    match state
        .tasks
        .delete_push_config(&params.id, params.push_notification_config_id.as_deref())
        .await
    {
        Ok(removed) => {
            // Best-effort: tear down any in-flight webhook subscribers too.
            state
                .push
                .unregister(&params.id, params.push_notification_config_id.as_deref());
            Json(A2aResponse::ok(
                id,
                serde_json::json!({ "removed": removed }),
            ))
            .into_response()
        }
        Err(e) => Json(A2aResponse::err(
            id,
            A2aError::new(A2aError::INTERNAL_ERROR, e.to_string()),
        ))
        .into_response(),
    }
}

fn sse_result(id: &Option<Value>, result: StreamingMessageResult) -> SseEvent {
    let env = A2aResponse::ok(
        id.clone(),
        serde_json::to_value(&result).unwrap_or(Value::Null),
    );
    sse_frame(&env)
}

fn sse_frame(env: &A2aResponse) -> SseEvent {
    SseEvent::default().data(serde_json::to_string(env).unwrap_or_default())
}

/// Start a task from a `message/*` call. Returns the created (Submitted →
/// Working) task and the runner's event stream.
async fn start_task(
    state: &A2aState,
    params: &MessageSendParams,
) -> Result<(Task, crate::core::EventStream<'static>)> {
    let user_msg = params.message.clone();
    let user_text_role = matches!(user_msg.role, MessageRole::User);
    if !user_text_role {
        return Err(crate::error::Error::config(
            "message.role must be \"user\" for an inbound request",
        ));
    }
    // ContextId becomes the session id; if missing, a fresh session is
    // created by the Runner.
    let context_id = user_msg
        .context_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let task_id = Uuid::new_v4().to_string();
    let initial_history = vec![Message {
        context_id: Some(context_id.clone()),
        task_id: Some(task_id.clone()),
        ..user_msg.clone()
    }];
    let task = state
        .tasks
        .create_task(new_task(
            task_id.clone(),
            context_id.clone(),
            initial_history,
        ))
        .await?;
    // Transition to Working before we start the agent.
    state
        .tasks
        .update_status(
            &task_id,
            TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Some(rfc3339_now()),
            },
            false,
        )
        .await?;
    let user_content = Content {
        role: crate::genai_types::Role::User,
        parts: user_msg.parts.iter().map(a2a_part_to_adk).collect(),
    };
    let user_id = user_msg
        .metadata
        .as_ref()
        .and_then(|m| m.get("user_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("anonymous")
        .to_string();
    let handle = state
        .runner
        .start(
            &user_id,
            Some(&context_id),
            user_content,
            RunConfig::default(),
        )
        .await?;
    // Record the runner invocation id on the task so `tasks/cancel` can
    // route the cancel back to the in-flight runner. Stored in the task's
    // metadata under `adk:invocationId`; the spec leaves `metadata`
    // free-form so this is namespaced to avoid collisions with caller-set
    // keys.
    let mut task = task;
    let mut md = task.metadata.clone().unwrap_or_default();
    md.insert(
        "adk:invocationId".into(),
        serde_json::Value::String(handle.invocation_id.clone()),
    );
    task.metadata = Some(md);
    // Persist the metadata update so `tasks/get` and later cancels can see
    // it. The in-memory TaskService stores the whole task by value, so we
    // need to rewrite it.
    state.tasks.create_task(task.clone()).await?;
    Ok((task, handle.events))
}

/// Side-effect plan emitted by [`Aggregator::absorb`] to be applied after
/// dropping any held lock.
enum PendingOp {
    AppendHistory { message: Message },
    AppendArtifact { artifact: Artifact },
}

impl PendingOp {
    async fn apply(self, state: &A2aState) {
        match self {
            Self::AppendHistory { message } => {
                let task_id = message.task_id.clone().unwrap_or_default();
                let _ = state.tasks.append_history(&task_id, message).await;
            }
            Self::AppendArtifact { artifact } => {
                let task_id_hint = artifact
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("taskId"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if let Some(tid) = task_id_hint {
                    let _ = state.tasks.append_artifact(&tid, artifact).await;
                }
            }
        }
    }
}

/// Accumulates artifact chunks while a single run streams in.
struct Aggregator {
    task_id: String,
    context_id: String,
    artifact_id: String,
    seen_text: bool,
    pending: Vec<PendingOp>,
}

impl Aggregator {
    fn new(task_id: String, context_id: String) -> Self {
        let artifact_id = format!("response-{}", Uuid::new_v4());
        Self {
            task_id,
            context_id,
            artifact_id,
            seen_text: false,
            pending: Vec::new(),
        }
    }

    fn drain_pending(&mut self) -> Vec<PendingOp> {
        std::mem::take(&mut self.pending)
    }

    /// Translate one ADK [`Event`] into pending writes to the task service.
    /// The actual writes are deferred until `drain_pending` to keep the
    /// lock-scope minimal.
    fn absorb(&mut self, ev: &Event) {
        // Skip user echoes (the runner re-emits the inbound user content as
        // an event; that's already in history).
        if ev.author == "user" {
            return;
        }
        if let Some(msg) = event_to_message(ev, &self.context_id, &self.task_id) {
            // Add the message to history first.
            self.pending.push(PendingOp::AppendHistory {
                message: msg.clone(),
            });
            // For text content, also emit an artifact chunk so callers can
            // render output as it arrives.
            let text: String = msg
                .parts
                .iter()
                .filter_map(|p| {
                    if let Part::Text { text, .. } = p {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            if !text.is_empty() {
                let mut artifact = artifact_chunk(
                    &self.artifact_id,
                    &text,
                    /*append=*/ self.seen_text,
                    /*last_chunk=*/ ev.turn_complete == Some(true),
                );
                // Carry the task id through metadata so the eventual
                // `append_artifact` knows where it goes (the
                // `apply` flow runs outside the lock that knows
                // which task we're in).
                let mut md = indexmap::IndexMap::new();
                md.insert(
                    "taskId".to_string(),
                    serde_json::Value::String(self.task_id.clone()),
                );
                artifact.metadata = Some(md);
                self.seen_text = true;
                self.pending.push(PendingOp::AppendArtifact { artifact });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::LlmAgent;
    use crate::core::Model;
    use crate::core::testing::MockModel;
    use crate::services::mem::InMemorySessionService;
    use serde_json::json;

    fn build_runner() -> Arc<Runner> {
        let model = Arc::new(MockModel::new("mock"));
        model.push_text("hello from remote");
        let agent = Arc::new(
            LlmAgent::builder("greeter")
                .model(model.clone() as Arc<dyn Model>)
                .instruction("greet")
                .build()
                .unwrap(),
        );
        Arc::new(
            Runner::builder()
                .app_name("a2a-test")
                .agent(agent)
                .session_service(Arc::new(InMemorySessionService::new()))
                .auto_create_session(true)
                .build()
                .unwrap(),
        )
    }

    fn card() -> AgentCard {
        AgentCard {
            name: "greeter".into(),
            description: "greets".into(),
            url: "http://localhost/".into(),
            provider: None,
            version: "0.1.0".into(),
            documentation_url: None,
            capabilities: super::super::types::AgentCapabilities {
                streaming: true,
                push_notifications: false,
                state_transition_history: false,
            },
            authentication: None,
            default_input_modes: vec!["text/plain".into()],
            default_output_modes: vec!["text/plain".into()],
            skills: vec![],
        }
    }

    fn state(runner: Arc<Runner>) -> A2aState {
        A2aState::new(runner, A2aServerConfig::new(card()))
    }

    #[tokio::test]
    async fn message_send_creates_completed_task_with_history_and_artifact() {
        let st = state(build_runner());
        let params = MessageSendParams {
            message: Message::user_text("hi"),
            configuration: None,
            metadata: None,
        };
        let resp = handle_message_send(
            st.clone(),
            Some(Value::String("1".into())),
            Some(serde_json::to_value(&params).unwrap()),
        )
        .await;
        let body = body_bytes(resp).await;
        let env: A2aResponse = serde_json::from_slice(&body).unwrap();
        assert!(env.error.is_none(), "got error: {:?}", env.error);
        let task: Task = serde_json::from_value(env.result.unwrap()).unwrap();
        assert_eq!(task.status.state, TaskState::Completed);
        // History should carry at least the user message + the agent reply.
        assert!(task.history.len() >= 2);
        // Artifact accumulates the agent's text reply.
        assert!(
            task.artifacts.iter().any(|a| {
                a.parts.iter().any(
                    |p| matches!(p, Part::Text { text, .. } if text.contains("hello from remote")),
                )
            }),
            "expected artifact containing the agent text, got {:?}",
            task.artifacts
        );
    }

    #[tokio::test]
    async fn tasks_get_returns_known_task() {
        let st = state(build_runner());
        // Drive a task first.
        let _ = handle_message_send(
            st.clone(),
            None,
            Some(
                serde_json::to_value(&MessageSendParams {
                    message: Message::user_text("hi"),
                    configuration: None,
                    metadata: None,
                })
                .unwrap(),
            ),
        )
        .await;
        // Look up any task currently in the store.
        let tid = {
            let all = st.tasks.clone();
            // The InMemory store doesn't expose `list`; use Tasks/Get over
            // a known id via re-creating a task.
            let new_t = all
                .create_task(new_task("probe".into(), "ctx".into(), vec![]))
                .await
                .unwrap();
            new_t.id
        };
        let resp = handle_tasks_get(
            st,
            Some(Value::String("2".into())),
            Some(
                serde_json::to_value(&TaskQueryParams {
                    id: tid,
                    history_length: None,
                })
                .unwrap(),
            ),
        )
        .await;
        let body = body_bytes(resp).await;
        let env: A2aResponse = serde_json::from_slice(&body).unwrap();
        assert!(env.error.is_none());
    }

    #[tokio::test]
    async fn tasks_cancel_flips_to_canceled() {
        let st = state(build_runner());
        st.tasks
            .create_task(new_task("t-1".into(), "ctx".into(), vec![]))
            .await
            .unwrap();
        let resp = handle_tasks_cancel(
            st,
            Some(Value::String("1".into())),
            Some(serde_json::to_value(&TaskIdParams { id: "t-1".into() }).unwrap()),
        )
        .await;
        let body = body_bytes(resp).await;
        let env: A2aResponse = serde_json::from_slice(&body).unwrap();
        let t: Task = serde_json::from_value(env.result.unwrap()).unwrap();
        assert_eq!(t.status.state, TaskState::Canceled);
    }

    #[tokio::test]
    async fn tasks_get_unknown_id_returns_task_not_found() {
        let st = state(build_runner());
        let resp = handle_tasks_get(
            st,
            Some(Value::String("1".into())),
            Some(
                serde_json::to_value(&TaskQueryParams {
                    id: "no-such-task".into(),
                    history_length: None,
                })
                .unwrap(),
            ),
        )
        .await;
        let body = body_bytes(resp).await;
        let env: A2aResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(env.error.unwrap().code, A2aError::TASK_NOT_FOUND);
    }

    #[tokio::test]
    async fn rpc_unknown_method_returns_method_not_found() {
        let st = state(build_runner());
        let resp = rpc_handler(
            State(st),
            Json(A2aRequest {
                jsonrpc: "2.0".into(),
                id: Some(Value::String("1".into())),
                method: "bogus/method".into(),
                params: None,
            }),
        )
        .await;
        let body = body_bytes(resp).await;
        let env: A2aResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(env.error.unwrap().code, A2aError::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn agent_card_endpoint_serves_the_card() {
        use axum::body::Body;
        use axum::http::{Method, Request};
        use tower::ServiceExt;

        let st = state(build_runner());
        let app = router(st);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/.well-known/agent.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_bytes(resp).await;
        let card: AgentCard = serde_json::from_slice(&body).unwrap();
        assert_eq!(card.name, "greeter");
        assert!(card.capabilities.streaming);
    }

    #[tokio::test]
    async fn agent_card_endpoint_uses_configured_path() {
        use axum::body::Body;
        use axum::http::{Method, Request};
        use tower::ServiceExt;

        let mut cfg = A2aServerConfig::new(card());
        cfg.agent_card_path = "/agent-card.json".into();
        let st = A2aState::new(build_runner(), cfg);
        let app = router(st);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/agent-card.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_required_when_token_set() {
        use axum::body::Body;
        use axum::http::{Method, Request};
        use tower::ServiceExt;

        let cfg = A2aServerConfig::new(card()).with_bearer_token("hunter2");
        let st = A2aState::new(build_runner(), cfg);
        let app = router(st);
        let payload = json!({
            "jsonrpc":"2.0","id":"1","method":"message/send",
            "params":{"message":{"kind":"message","role":"user","messageId":"m1","parts":[{"kind":"text","text":"hi"}]}}
        });
        // No token → 401.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // Correct token → 200.
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .header("content-type", "application/json")
                    .header(header::AUTHORIZATION, "Bearer hunter2")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn serve_refuses_non_loopback_without_auth() {
        use std::net::{IpAddr, Ipv4Addr};
        let st = state(build_runner());
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
        let err = serve(addr, st).await.unwrap_err();
        assert!(err.to_string().contains("non-loopback"));
    }

    fn push_set_params(task_id: &str, url: &str) -> Value {
        serde_json::to_value(&TaskPushNotificationConfig {
            task_id: task_id.into(),
            push_notification_config: crate::a2a::types::PushNotificationConfig {
                id: None,
                url: url.into(),
                token: None,
                authentication: None,
            },
        })
        .unwrap()
    }

    #[tokio::test]
    async fn push_config_set_get_list_delete_via_rpc() {
        let st = state(build_runner());
        st.tasks
            .create_task(new_task("t-1".into(), "ctx".into(), vec![]))
            .await
            .unwrap();

        // set
        let resp = handle_push_config_set(
            st.clone(),
            Some(Value::String("1".into())),
            Some(push_set_params("t-1", "https://hooks.example.com/cb")),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        assert!(env.error.is_none(), "got error: {:?}", env.error);
        let stored: TaskPushNotificationConfig =
            serde_json::from_value(env.result.unwrap()).unwrap();
        assert!(stored.push_notification_config.id.is_some());
        let config_id = stored.push_notification_config.id.clone().unwrap();

        // get by id
        let resp = handle_push_config_get(
            st.clone(),
            Some(Value::String("2".into())),
            Some(
                serde_json::to_value(&GetTaskPushNotificationConfigParams {
                    id: "t-1".into(),
                    push_notification_config_id: Some(config_id.clone()),
                })
                .unwrap(),
            ),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        assert!(env.error.is_none());

        // list
        let resp = handle_push_config_list(
            st.clone(),
            Some(Value::String("3".into())),
            Some(serde_json::to_value(&TaskIdParams { id: "t-1".into() }).unwrap()),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        let list: Vec<TaskPushNotificationConfig> =
            serde_json::from_value(env.result.unwrap()).unwrap();
        assert_eq!(list.len(), 1);

        // delete by id
        let resp = handle_push_config_delete(
            st.clone(),
            Some(Value::String("4".into())),
            Some(
                serde_json::to_value(&GetTaskPushNotificationConfigParams {
                    id: "t-1".into(),
                    push_notification_config_id: Some(config_id),
                })
                .unwrap(),
            ),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        assert_eq!(env.result.unwrap()["removed"], 1);

        // list again → empty
        let resp = handle_push_config_list(
            st,
            Some(Value::String("5".into())),
            Some(serde_json::to_value(&TaskIdParams { id: "t-1".into() }).unwrap()),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        let list: Vec<TaskPushNotificationConfig> =
            serde_json::from_value(env.result.unwrap()).unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn push_config_set_rejects_plaintext_http_url() {
        let st = state(build_runner());
        st.tasks
            .create_task(new_task("t-1".into(), "ctx".into(), vec![]))
            .await
            .unwrap();
        let resp = handle_push_config_set(
            st,
            Some(Value::String("1".into())),
            Some(push_set_params("t-1", "http://attacker.example.com/cb")),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        let err = env.error.expect("expected INVALID_PARAMS");
        assert_eq!(err.code, A2aError::INVALID_PARAMS);
        assert!(err.message.to_lowercase().contains("https"));
    }

    #[tokio::test]
    async fn push_config_set_returns_task_not_found_for_unknown_task() {
        let st = state(build_runner());
        let resp = handle_push_config_set(
            st,
            Some(Value::String("1".into())),
            Some(push_set_params("missing", "https://hooks.example.com/cb")),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        assert_eq!(env.error.unwrap().code, A2aError::TASK_NOT_FOUND);
    }

    #[tokio::test]
    async fn tasks_cancel_propagates_to_runner_invocation() {
        // Build a runner whose agent would emit text after a function-call
        // round-trip; we cancel via the A2A `tasks/cancel` handler before
        // polling the stream and verify the agent observes the flag.
        use crate::core::testing::MockModel;
        use crate::services::mem::InMemorySessionService;

        let model = Arc::new(MockModel::new("mock"));
        // Two turns queued — if the agent ran, the runner would emit them.
        model.push_text("turn-1");
        model.push_text("turn-2");
        let agent = Arc::new(
            LlmAgent::builder("a")
                .model(model.clone() as Arc<dyn Model>)
                .instruction("be terse")
                .build()
                .unwrap(),
        );
        let runner = Arc::new(
            Runner::builder()
                .app_name("a2a-cancel-test")
                .agent(agent)
                .session_service(Arc::new(InMemorySessionService::new()))
                .auto_create_session(true)
                .build()
                .unwrap(),
        );
        let st = A2aState::new(runner.clone(), A2aServerConfig::new(card()));

        // Start a task via `start_task` (so the invocation_id ends up in the
        // task's metadata).
        let params = MessageSendParams {
            message: Message::user_text("hi"),
            configuration: None,
            metadata: None,
        };
        let (task, stream) = start_task(&st, &params).await.unwrap();
        let task_id = task.id.clone();
        let inv_id = task
            .metadata
            .as_ref()
            .and_then(|m| m.get("adk:invocationId"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .expect("start_task should stash adk:invocationId on the task");
        assert!(runner.is_active(&inv_id), "runner should know this id");

        // Cancel via the A2A handler — this must flip the runner's
        // cancellation token AND mark the task `canceled`.
        let resp = handle_tasks_cancel(
            st.clone(),
            Some(Value::String("c-1".into())),
            Some(
                serde_json::to_value(&TaskIdParams {
                    id: task_id.clone(),
                })
                .unwrap(),
            ),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        assert!(env.error.is_none(), "cancel rpc failed: {:?}", env.error);
        let t: Task = serde_json::from_value(env.result.unwrap()).unwrap();
        assert_eq!(t.status.state, TaskState::Canceled);

        // Drain the originally-returned stream — the agent should observe
        // the cancellation and emit at most a CANCELLED marker, never
        // "turn-1" / "turn-2".
        let events = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(
            !events.iter().any(|e| {
                e.response
                    .content
                    .as_ref()
                    .map(|c| c.text_concat().contains("turn-"))
                    .unwrap_or(false)
            }),
            "agent emitted text after A2A cancel"
        );
    }

    #[tokio::test]
    async fn end_to_end_webhook_fires_on_status_update() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{method as m, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let webhook = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();
        Mock::given(m("POST"))
            .and(path("/hook"))
            .respond_with(move |_req: &wiremock::Request| {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .mount(&webhook)
            .await;

        // Build state with a real loopback webhook URL.
        let st = state(build_runner());
        st.tasks
            .create_task(new_task("t-1".into(), "ctx".into(), vec![]))
            .await
            .unwrap();

        // Register the webhook via the RPC handler so the full path runs.
        let resp = handle_push_config_set(
            st.clone(),
            Some(Value::String("1".into())),
            Some(push_set_params("t-1", &format!("{}/hook", webhook.uri()))),
        )
        .await;
        let env: A2aResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        assert!(env.error.is_none(), "register failed: {:?}", env.error);

        // Drive a status update on the task.
        st.tasks
            .update_status(
                "t-1",
                TaskStatus {
                    state: TaskState::Working,
                    message: None,
                    timestamp: None,
                },
                false,
            )
            .await
            .unwrap();

        // Give the spawned subscriber a brief window to deliver.
        for _ in 0..50 {
            if calls.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "webhook never fired; got {} hits",
            calls.load(Ordering::SeqCst)
        );
    }

    async fn body_bytes(resp: axum::response::Response) -> axum::body::Bytes {
        use http_body_util::BodyExt;
        resp.into_body().collect().await.unwrap().to_bytes()
    }
}
