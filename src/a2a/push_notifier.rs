//! Outbound webhook delivery for `tasks/pushNotificationConfig/*`.
//!
//! On each [`TaskService`] update, [`PushNotifier`] fans the update out to
//! every webhook config registered for that task. Delivery is best-effort
//! and fire-and-forget: failures are logged at `warn`, retried with a tiny
//! bounded backoff (3 attempts), and never propagated back to the inbound
//! caller — A2A's push semantics treat the SSE/`tasks/get` channel as the
//! authoritative one.
//!
//! Webhook bodies match what `message/stream` emits over SSE:
//!
//! ```json
//! {
//!   "jsonrpc": "2.0",
//!   "result": <TaskStatusUpdateEvent | TaskArtifactUpdateEvent>
//! }
//! ```
//!
//! so a receiver written for `message/stream` can consume them without
//! ceremony.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::error::Result;

use super::task_service::{TaskService, TaskUpdate};
use super::types::{A2aResponse, PushNotificationConfig, StreamingMessageResult};

/// `(task_id, config_id) -> JoinHandle`. Aborting the handle stops a
/// subscriber loop.
type SubscriberMap = Arc<Mutex<HashMap<(String, String), JoinHandle<()>>>>;

/// Fans task updates out to registered webhook URLs.
///
/// One [`PushNotifier`] is owned by each [`crate::a2a::A2aState`]; it shares
/// the same `Arc<dyn TaskService>` so it can resolve configs and subscribe
/// to update broadcasts. Subscribers are spawned per-config and cleaned up
/// on `unregister`, on task termination, or when the [`PushNotifier`]
/// itself is dropped.
pub struct PushNotifier {
    tasks: Arc<dyn TaskService>,
    http: reqwest::Client,
    active: SubscriberMap,
}

impl std::fmt::Debug for PushNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.active.lock().len();
        f.debug_struct("PushNotifier")
            .field("active_subscribers", &n)
            .finish_non_exhaustive()
    }
}

impl PushNotifier {
    /// Construct a notifier sharing `tasks` and using a default HTTP client
    /// (`10s` per-request timeout).
    pub fn new(tasks: Arc<dyn TaskService>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("adk-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| crate::error::Error::other(format!("push http client: {e}")))?;
        Ok(Self {
            tasks,
            http,
            active: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Begin fanning task updates to `config.url`. Idempotent: a previous
    /// subscriber for the same `(task_id, config.id)` is aborted first.
    /// Silently no-ops if the task is unknown or already terminal — the
    /// caller already learned that from
    /// [`TaskService::set_push_config`]'s return value.
    pub async fn register(&self, task_id: &str, config: &PushNotificationConfig) {
        let config_id = match config.id.as_deref() {
            Some(id) => id.to_string(),
            None => {
                warn!("PushNotifier::register called with config.id=None — skipping");
                return;
            }
        };
        let key = (task_id.to_string(), config_id.clone());
        if let Some(prev) = self.active.lock().remove(&key) {
            prev.abort();
        }

        let rx = match self.tasks.subscribe(task_id).await {
            Ok(Some(rx)) => rx,
            Ok(None) => return,
            Err(e) => {
                warn!("push subscribe({task_id}) failed: {e}");
                return;
            }
        };

        let http = self.http.clone();
        let url = config.url.clone();
        let token = config.token.clone();
        let active = self.active.clone();
        let key_for_cleanup = key.clone();
        let handle = tokio::spawn(async move {
            run_subscriber(http, url, token, rx).await;
            // Self-cleanup so the active map doesn't grow unbounded after
            // the task terminates.
            active.lock().remove(&key_for_cleanup);
        });
        self.active.lock().insert(key, handle);
    }

    /// Stop fanning updates for one config (or every config registered
    /// against `task_id` if `config_id` is `None`). Returns the number of
    /// subscribers stopped.
    pub fn unregister(&self, task_id: &str, config_id: Option<&str>) -> usize {
        let mut active = self.active.lock();
        let keys: Vec<(String, String)> = active
            .keys()
            .filter(|(tid, cid)| {
                tid == task_id && config_id.is_none_or(|want| want == cid.as_str())
            })
            .cloned()
            .collect();
        let mut stopped = 0;
        for k in keys {
            if let Some(handle) = active.remove(&k) {
                handle.abort();
                stopped += 1;
            }
        }
        stopped
    }

    /// True if a subscriber is currently running for `(task_id, config_id)`.
    /// Used by tests; not part of the spec surface.
    #[must_use]
    pub fn has_subscriber(&self, task_id: &str, config_id: &str) -> bool {
        self.active
            .lock()
            .contains_key(&(task_id.to_string(), config_id.to_string()))
    }
}

impl Drop for PushNotifier {
    fn drop(&mut self) {
        let mut active = self.active.lock();
        for (_, handle) in active.drain() {
            handle.abort();
        }
    }
}

async fn run_subscriber(
    http: reqwest::Client,
    url: String,
    token: Option<String>,
    mut rx: tokio::sync::broadcast::Receiver<TaskUpdate>,
) {
    let headers = build_headers(token.as_deref());
    loop {
        match rx.recv().await {
            Ok(update) => {
                let envelope = A2aResponse::ok(
                    None,
                    serde_json::to_value(result_from_update(&update)).unwrap_or_default(),
                );
                deliver(&http, &url, &headers, &envelope).await;
                if is_final(&update) {
                    break;
                }
            }
            Err(RecvError::Closed) => break,
            // Lagged: skip and keep going. Webhook receivers re-sync via
            // `tasks/get` anyway.
            Err(RecvError::Lagged(_)) => continue,
        }
    }
}

fn result_from_update(u: &TaskUpdate) -> StreamingMessageResult {
    match u {
        TaskUpdate::Status(s) => StreamingMessageResult::Status(s.clone()),
        TaskUpdate::Artifact(a) => StreamingMessageResult::Artifact(a.clone()),
    }
}

fn is_final(u: &TaskUpdate) -> bool {
    matches!(u, TaskUpdate::Status(s) if s.is_final)
}

fn build_headers(token: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(t) = token {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
            h.insert(AUTHORIZATION, v);
        }
    }
    h
}

async fn deliver(http: &reqwest::Client, url: &str, headers: &HeaderMap, envelope: &A2aResponse) {
    let body = match serde_json::to_vec(envelope) {
        Ok(b) => b,
        Err(e) => {
            warn!("push: encode envelope: {e}");
            return;
        }
    };
    // Tiny bounded retry budget so a flaky receiver doesn't drop notices
    // we care about, but a wedged receiver doesn't block the dispatch
    // pipeline either.
    const ATTEMPTS: usize = 3;
    for attempt in 1..=ATTEMPTS {
        let resp = http
            .post(url)
            .headers(headers.clone())
            .body(body.clone())
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                debug!(%url, status = r.status().as_u16(), "push delivered");
                return;
            }
            Ok(r) => {
                warn!(
                    %url, status = r.status().as_u16(), attempt,
                    "push delivery returned non-2xx"
                );
            }
            Err(e) => {
                warn!(%url, attempt, "push delivery error: {e}");
            }
        }
        // Backoff: 100ms, 400ms — short enough not to back up the inbound
        // pipeline since this runs in its own spawned task.
        if attempt < ATTEMPTS {
            let delay = Duration::from_millis(100 * (attempt as u64).pow(2));
            tokio::time::sleep(delay).await;
        }
    }
    warn!(%url, "push delivery giving up after {ATTEMPTS} attempts");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::task_service::InMemoryTaskService;
    use crate::a2a::types::{Artifact, Part, TaskKind, TaskState, TaskStatus};

    fn fake_task(id: &str) -> super::super::types::Task {
        super::super::types::Task {
            kind: TaskKind::Task,
            id: id.into(),
            context_id: format!("ctx-{id}"),
            status: TaskStatus {
                state: TaskState::Submitted,
                message: None,
                timestamp: None,
            },
            artifacts: vec![],
            history: vec![],
            metadata: None,
        }
    }

    #[tokio::test]
    async fn unregister_aborts_subscriber() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Server isn't called in this test — we just need a URL that
        // exists so the subscriber loop can start. Any 2xx is fine.
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let tasks: Arc<dyn TaskService> = Arc::new(InMemoryTaskService::new());
        tasks.create_task(fake_task("t-1")).await.unwrap();
        let notifier = PushNotifier::new(tasks.clone()).unwrap();
        let cfg = PushNotificationConfig {
            id: Some("c-1".into()),
            url: format!("{}/hook", server.uri()),
            token: None,
            authentication: None,
        };
        notifier.register("t-1", &cfg).await;
        assert!(notifier.has_subscriber("t-1", "c-1"));
        let n = notifier.unregister("t-1", Some("c-1"));
        assert_eq!(n, 1);
        assert!(!notifier.has_subscriber("t-1", "c-1"));
    }

    #[tokio::test]
    async fn delivers_status_update_to_webhook() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let resp = ResponseTemplate::new(200)
            .set_body_string("ok")
            // Increment the call counter on each invocation.
            .insert_header("x-test", "ok");
        Mock::given(method("POST"))
            .and(path("/hook"))
            .and(header("authorization", "Bearer my-token"))
            .respond_with(move |_req: &wiremock::Request| {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                resp.clone()
            })
            .mount(&server)
            .await;

        let tasks: Arc<dyn TaskService> = Arc::new(InMemoryTaskService::new());
        tasks.create_task(fake_task("t-1")).await.unwrap();
        let notifier = PushNotifier::new(tasks.clone()).unwrap();
        let cfg = PushNotificationConfig {
            id: Some("c-1".into()),
            url: format!("{}/hook", server.uri()),
            token: Some("my-token".into()),
            authentication: None,
        };
        notifier.register("t-1", &cfg).await;

        // Trigger an update.
        tasks
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

        // Wait briefly for the spawned subscriber to deliver.
        for _ in 0..50 {
            if calls.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "expected webhook to be called, got {} hits",
            calls.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn subscriber_self_cleans_after_terminal_status() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let tasks: Arc<dyn TaskService> = Arc::new(InMemoryTaskService::new());
        tasks.create_task(fake_task("t-1")).await.unwrap();
        let notifier = PushNotifier::new(tasks.clone()).unwrap();
        let cfg = PushNotificationConfig {
            id: Some("c-1".into()),
            url: format!("{}/hook", server.uri()),
            token: None,
            authentication: None,
        };
        notifier.register("t-1", &cfg).await;

        tasks
            .update_status(
                "t-1",
                TaskStatus {
                    state: TaskState::Completed,
                    message: None,
                    timestamp: None,
                },
                /*is_final=*/ true,
            )
            .await
            .unwrap();

        // Wait briefly for the subscriber's self-cleanup.
        for _ in 0..50 {
            if !notifier.has_subscriber("t-1", "c-1") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!notifier.has_subscriber("t-1", "c-1"));
    }

    #[tokio::test]
    async fn body_carries_jsonrpc_envelope_with_streaming_result() {
        use parking_lot::Mutex as PLMutex;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let captured: Arc<PLMutex<Option<serde_json::Value>>> = Arc::new(PLMutex::new(None));
        let captured_clone = captured.clone();
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(move |req: &wiremock::Request| {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&req.body) {
                    *captured_clone.lock() = Some(v);
                }
                ResponseTemplate::new(204)
            })
            .mount(&server)
            .await;

        let tasks: Arc<dyn TaskService> = Arc::new(InMemoryTaskService::new());
        tasks.create_task(fake_task("t-1")).await.unwrap();
        let notifier = PushNotifier::new(tasks.clone()).unwrap();
        notifier
            .register(
                "t-1",
                &PushNotificationConfig {
                    id: Some("c-1".into()),
                    url: format!("{}/hook", server.uri()),
                    token: None,
                    authentication: None,
                },
            )
            .await;

        // Fire an artifact update; richer than a status update for shape checks.
        let artifact = Artifact {
            artifact_id: "a-1".into(),
            name: None,
            description: None,
            parts: vec![Part::text("hi")],
            index: None,
            append: None,
            last_chunk: Some(true),
            metadata: None,
        };
        tasks.append_artifact("t-1", artifact).await.unwrap();

        for _ in 0..50 {
            if captured.lock().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let body = captured.lock().clone().expect("webhook captured a body");
        assert_eq!(body["jsonrpc"], "2.0");
        // Result is a TaskArtifactUpdateEvent (kind: "artifact-update").
        assert_eq!(body["result"]["kind"], "artifact-update");
        assert_eq!(body["result"]["artifact"]["artifactId"], "a-1");
    }
}
