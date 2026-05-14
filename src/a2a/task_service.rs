//! [`TaskService`] — pluggable persistence for A2A [`Task`] state.
//!
//! A2A tasks have a richer lifecycle than ADK sessions: they're addressed
//! by an external id, transition through a small state machine, accumulate
//! artifacts in addition to message history, and can be queried or canceled
//! after the original SSE channel has closed. That justifies a dedicated
//! trait rather than wedging tasks into `SessionService`.
//!
//! The default implementation is in-memory and lives in this same module
//! ([`InMemoryTaskService`]). External implementations (Redis, SQL, …) can
//! implement [`TaskService`] directly.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::error::Result;

use super::types::{
    Artifact, Message, PushNotificationConfig, Task, TaskArtifactUpdateEvent, TaskKind, TaskState,
    TaskStatus, TaskStatusUpdateEvent,
};

/// A live update on a task's SSE channel. Both `message/stream` callers
/// (the original requester) and `tasks/resubscribe` callers receive the
/// same stream, fanned out per-subscriber.
#[derive(Debug, Clone)]
pub enum TaskUpdate {
    /// Status transition.
    Status(TaskStatusUpdateEvent),
    /// New (or appended) artifact.
    Artifact(TaskArtifactUpdateEvent),
}

/// Persistence + fan-out for A2A tasks.
///
/// Implementations must be cheap to clone (typically `Arc`'d internal
/// state) so a single [`TaskService`] instance can be shared by all
/// concurrent requests against an A2A server.
#[async_trait]
pub trait TaskService: Send + Sync + std::fmt::Debug + 'static {
    /// Insert a fresh task in [`TaskState::Submitted`]. Returns the task as
    /// stored (with `kind: "task"`).
    async fn create_task(&self, task: Task) -> Result<Task>;

    /// Look up a task by id. `history_length`, if provided, trims the
    /// returned history to the most recent `n` entries.
    async fn get_task(&self, id: &str, history_length: Option<u32>) -> Result<Option<Task>>;

    /// Mutate a task's status and broadcast a [`TaskStatusUpdateEvent`] to
    /// every active subscriber. `is_final` is forwarded onto the update —
    /// subscribers use it to close their SSE channel.
    async fn update_status(
        &self,
        id: &str,
        new_status: TaskStatus,
        is_final: bool,
    ) -> Result<Option<Task>>;

    /// Append a message to a task's `history`. Used for both user messages
    /// (when `tasks/send`-style continuations arrive) and agent messages
    /// emitted during streaming.
    async fn append_history(&self, id: &str, message: Message) -> Result<Option<Task>>;

    /// Append (or replace, depending on `artifact.append`) an artifact and
    /// broadcast a [`TaskArtifactUpdateEvent`].
    async fn append_artifact(&self, id: &str, artifact: Artifact) -> Result<Option<Task>>;

    /// Subscribe to the live update stream for `id`. Returns `None` if the
    /// task is unknown or already terminal (callers fall back on
    /// [`Self::get_task`] in that case).
    ///
    /// Receivers may miss updates emitted before they subscribe — that's
    /// the standard `tokio::sync::broadcast` lag semantics; callers should
    /// pair `subscribe` with a `get_task` call to grab the current state.
    async fn subscribe(&self, id: &str) -> Result<Option<broadcast::Receiver<TaskUpdate>>>;

    /// Mark a task as canceled. Returns
    /// `Err(crate::error::Error::Service(_))` if the task is already in a
    /// terminal state.
    async fn cancel_task(&self, id: &str) -> Result<Option<Task>>;

    /// Register a webhook config against a task. If `config.id` is `None`,
    /// the service assigns one (a UUID by default). Returns the stored
    /// config — with `id` populated — on success, or `Ok(None)` if the
    /// task is unknown.
    async fn set_push_config(
        &self,
        task_id: &str,
        config: PushNotificationConfig,
    ) -> Result<Option<PushNotificationConfig>>;

    /// Look up a registered push config. When `config_id` is `None`, returns
    /// the first config for the task (mirrors how the spec's `get` method
    /// degrades when the id is omitted).
    async fn get_push_config(
        &self,
        task_id: &str,
        config_id: Option<&str>,
    ) -> Result<Option<PushNotificationConfig>>;

    /// List every registered push config for `task_id`. Empty if the task
    /// is unknown or has no configs.
    async fn list_push_configs(&self, task_id: &str) -> Result<Vec<PushNotificationConfig>>;

    /// Remove a push config. When `config_id` is `None`, all configs for
    /// the task are removed (mirrors the spec's `delete` semantics).
    /// Returns the number of configs removed.
    async fn delete_push_config(&self, task_id: &str, config_id: Option<&str>) -> Result<usize>;
}

/// In-memory [`TaskService`] — the default. Stores tasks behind a single
/// mutex and fans out updates via a per-task `broadcast::Sender`.
pub struct InMemoryTaskService {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    tasks: HashMap<String, Task>,
    senders: HashMap<String, broadcast::Sender<TaskUpdate>>,
    /// `task_id -> config_id -> config`. Stored separately from `tasks` so a
    /// future SQL impl can use a side table without touching the task row.
    push_configs: HashMap<String, HashMap<String, PushNotificationConfig>>,
}

impl std::fmt::Debug for InMemoryTaskService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryTaskService")
            .finish_non_exhaustive()
    }
}

impl Default for InMemoryTaskService {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryTaskService {
    /// New, empty service.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                tasks: HashMap::new(),
                senders: HashMap::new(),
                push_configs: HashMap::new(),
            })),
        }
    }

    fn sender_for(&self, id: &str) -> broadcast::Sender<TaskUpdate> {
        let mut inner = self.inner.lock();
        inner
            .senders
            .entry(id.to_string())
            .or_insert_with(|| broadcast::channel::<TaskUpdate>(32).0)
            .clone()
    }
}

#[async_trait]
impl TaskService for InMemoryTaskService {
    async fn create_task(&self, mut task: Task) -> Result<Task> {
        task.kind = TaskKind::Task;
        let mut inner = self.inner.lock();
        inner.tasks.insert(task.id.clone(), task.clone());
        inner
            .senders
            .entry(task.id.clone())
            .or_insert_with(|| broadcast::channel::<TaskUpdate>(32).0);
        Ok(task)
    }

    async fn get_task(&self, id: &str, history_length: Option<u32>) -> Result<Option<Task>> {
        let inner = self.inner.lock();
        let Some(mut task) = inner.tasks.get(id).cloned() else {
            return Ok(None);
        };
        if let Some(n) = history_length {
            let n = n as usize;
            if task.history.len() > n {
                let drop = task.history.len() - n;
                task.history.drain(..drop);
            }
        }
        Ok(Some(task))
    }

    async fn update_status(
        &self,
        id: &str,
        new_status: TaskStatus,
        is_final: bool,
    ) -> Result<Option<Task>> {
        let updated = {
            let mut inner = self.inner.lock();
            let Some(task) = inner.tasks.get_mut(id) else {
                return Ok(None);
            };
            task.status = new_status.clone();
            task.clone()
        };
        let evt = TaskStatusUpdateEvent {
            kind: super::types::StatusUpdateKind::StatusUpdate,
            task_id: updated.id.clone(),
            context_id: updated.context_id.clone(),
            status: new_status,
            is_final,
            metadata: None,
        };
        let tx = self.sender_for(id);
        // No receivers is not an error.
        let _ = tx.send(TaskUpdate::Status(evt));
        Ok(Some(updated))
    }

    async fn append_history(&self, id: &str, message: Message) -> Result<Option<Task>> {
        let mut inner = self.inner.lock();
        let Some(task) = inner.tasks.get_mut(id) else {
            return Ok(None);
        };
        task.history.push(message);
        Ok(Some(task.clone()))
    }

    async fn append_artifact(&self, id: &str, mut artifact: Artifact) -> Result<Option<Task>> {
        let updated = {
            let mut inner = self.inner.lock();
            let Some(task) = inner.tasks.get_mut(id) else {
                return Ok(None);
            };
            // If `append=true` and an artifact with the same id exists,
            // append the new parts; otherwise push.
            if artifact.append == Some(true) {
                if let Some(existing) = task
                    .artifacts
                    .iter_mut()
                    .find(|a| a.artifact_id == artifact.artifact_id)
                {
                    existing.parts.append(&mut artifact.parts);
                    existing.last_chunk = artifact.last_chunk.or(existing.last_chunk);
                    existing.clone()
                } else {
                    task.artifacts.push(artifact.clone());
                    artifact.clone()
                }
            } else {
                if let Some(pos) = task
                    .artifacts
                    .iter()
                    .position(|a| a.artifact_id == artifact.artifact_id)
                {
                    task.artifacts[pos] = artifact.clone();
                } else {
                    task.artifacts.push(artifact.clone());
                }
                artifact.clone()
            };
            task.clone()
        };
        let evt = TaskArtifactUpdateEvent {
            kind: super::types::ArtifactUpdateKind::ArtifactUpdate,
            task_id: updated.id.clone(),
            context_id: updated.context_id.clone(),
            artifact,
            metadata: None,
        };
        let _ = self.sender_for(id).send(TaskUpdate::Artifact(evt));
        Ok(Some(updated))
    }

    async fn subscribe(&self, id: &str) -> Result<Option<broadcast::Receiver<TaskUpdate>>> {
        let inner = self.inner.lock();
        let Some(task) = inner.tasks.get(id) else {
            return Ok(None);
        };
        if task.status.state.is_terminal() {
            return Ok(None);
        }
        // Touch the entry so a sender exists even if append_* hasn't run yet.
        let tx = match inner.senders.get(id) {
            Some(tx) => tx.clone(),
            None => {
                drop(inner);
                self.sender_for(id)
            }
        };
        Ok(Some(tx.subscribe()))
    }

    async fn set_push_config(
        &self,
        task_id: &str,
        mut config: PushNotificationConfig,
    ) -> Result<Option<PushNotificationConfig>> {
        let mut inner = self.inner.lock();
        if !inner.tasks.contains_key(task_id) {
            return Ok(None);
        }
        let config_id = config
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        config.id = Some(config_id.clone());
        inner
            .push_configs
            .entry(task_id.to_string())
            .or_default()
            .insert(config_id, config.clone());
        Ok(Some(config))
    }

    async fn get_push_config(
        &self,
        task_id: &str,
        config_id: Option<&str>,
    ) -> Result<Option<PushNotificationConfig>> {
        let inner = self.inner.lock();
        let Some(configs) = inner.push_configs.get(task_id) else {
            return Ok(None);
        };
        match config_id {
            Some(id) => Ok(configs.get(id).cloned()),
            None => Ok(configs.values().next().cloned()),
        }
    }

    async fn list_push_configs(&self, task_id: &str) -> Result<Vec<PushNotificationConfig>> {
        let inner = self.inner.lock();
        Ok(inner
            .push_configs
            .get(task_id)
            .map(|cs| cs.values().cloned().collect())
            .unwrap_or_default())
    }

    async fn delete_push_config(&self, task_id: &str, config_id: Option<&str>) -> Result<usize> {
        let mut inner = self.inner.lock();
        let Some(configs) = inner.push_configs.get_mut(task_id) else {
            return Ok(0);
        };
        match config_id {
            Some(id) => Ok(configs.remove(id).map(|_| 1).unwrap_or(0)),
            None => {
                let n = configs.len();
                configs.clear();
                Ok(n)
            }
        }
    }

    async fn cancel_task(&self, id: &str) -> Result<Option<Task>> {
        let (was_terminal, status) = {
            let inner = self.inner.lock();
            match inner.tasks.get(id) {
                None => return Ok(None),
                Some(t) => (
                    t.status.state.is_terminal(),
                    TaskStatus {
                        state: TaskState::Canceled,
                        message: None,
                        timestamp: Some(rfc3339_now()),
                    },
                ),
            }
        };
        if was_terminal {
            return Err(crate::error::Error::config(
                "task is already in a terminal state",
            ));
        }
        self.update_status(id, status, true).await
    }
}

/// RFC 3339 timestamp for the current wall-clock time.
#[must_use]
pub fn rfc3339_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::{Part, TaskState};

    fn new_task(id: &str) -> Task {
        Task {
            kind: TaskKind::Task,
            id: id.into(),
            context_id: format!("ctx-{id}"),
            status: TaskStatus {
                state: TaskState::Submitted,
                message: None,
                timestamp: Some(rfc3339_now()),
            },
            artifacts: vec![],
            history: vec![],
            metadata: None,
        }
    }

    #[tokio::test]
    async fn create_and_get_round_trip() {
        let svc = InMemoryTaskService::new();
        let t = svc.create_task(new_task("t-1")).await.unwrap();
        assert_eq!(t.id, "t-1");
        let got = svc.get_task("t-1", None).await.unwrap().unwrap();
        assert_eq!(got.status.state, TaskState::Submitted);
    }

    #[tokio::test]
    async fn update_status_broadcasts_to_subscribers() {
        let svc = InMemoryTaskService::new();
        svc.create_task(new_task("t-1")).await.unwrap();
        let mut sub = svc.subscribe("t-1").await.unwrap().unwrap();
        let new_status = TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: Some(rfc3339_now()),
        };
        svc.update_status("t-1", new_status.clone(), false)
            .await
            .unwrap();
        let upd = sub.recv().await.unwrap();
        match upd {
            TaskUpdate::Status(s) => assert_eq!(s.status.state, TaskState::Working),
            other => panic!("expected status update, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn append_artifact_broadcasts_and_persists() {
        let svc = InMemoryTaskService::new();
        svc.create_task(new_task("t-1")).await.unwrap();
        let mut sub = svc.subscribe("t-1").await.unwrap().unwrap();
        let artifact = Artifact {
            artifact_id: "a-1".into(),
            name: None,
            description: None,
            parts: vec![Part::text("hello")],
            index: None,
            append: None,
            last_chunk: Some(true),
            metadata: None,
        };
        svc.append_artifact("t-1", artifact.clone()).await.unwrap();
        let evt = sub.recv().await.unwrap();
        match evt {
            TaskUpdate::Artifact(a) => assert_eq!(a.artifact.artifact_id, "a-1"),
            other => panic!("expected artifact update, got {other:?}"),
        }
        let task = svc.get_task("t-1", None).await.unwrap().unwrap();
        assert_eq!(task.artifacts.len(), 1);
        assert_eq!(task.artifacts[0].artifact_id, "a-1");
    }

    #[tokio::test]
    async fn append_artifact_with_append_flag_grows_parts() {
        let svc = InMemoryTaskService::new();
        svc.create_task(new_task("t-1")).await.unwrap();
        svc.append_artifact(
            "t-1",
            Artifact {
                artifact_id: "a".into(),
                parts: vec![Part::text("hello ")],
                append: None,
                last_chunk: None,
                name: None,
                description: None,
                index: None,
                metadata: None,
            },
        )
        .await
        .unwrap();
        svc.append_artifact(
            "t-1",
            Artifact {
                artifact_id: "a".into(),
                parts: vec![Part::text("world")],
                append: Some(true),
                last_chunk: Some(true),
                name: None,
                description: None,
                index: None,
                metadata: None,
            },
        )
        .await
        .unwrap();
        let task = svc.get_task("t-1", None).await.unwrap().unwrap();
        assert_eq!(task.artifacts.len(), 1);
        assert_eq!(task.artifacts[0].parts.len(), 2);
        assert_eq!(task.artifacts[0].last_chunk, Some(true));
    }

    #[tokio::test]
    async fn cancel_task_flips_state_to_canceled() {
        let svc = InMemoryTaskService::new();
        svc.create_task(new_task("t-1")).await.unwrap();
        let t = svc.cancel_task("t-1").await.unwrap().unwrap();
        assert_eq!(t.status.state, TaskState::Canceled);
        // Second cancel must err: task is already terminal.
        let err = svc.cancel_task("t-1").await.unwrap_err();
        assert!(err.to_string().contains("terminal"));
    }

    #[tokio::test]
    async fn subscribe_returns_none_for_terminal_task() {
        let svc = InMemoryTaskService::new();
        svc.create_task(new_task("t-1")).await.unwrap();
        svc.cancel_task("t-1").await.unwrap();
        let sub = svc.subscribe("t-1").await.unwrap();
        assert!(sub.is_none());
    }

    #[tokio::test]
    async fn push_config_set_get_list_delete_round_trip() {
        let svc = InMemoryTaskService::new();
        svc.create_task(new_task("t-1")).await.unwrap();

        // set without id → server assigns one.
        let stored = svc
            .set_push_config(
                "t-1",
                PushNotificationConfig {
                    id: None,
                    url: "https://example.com/hook".into(),
                    token: Some("tok".into()),
                    authentication: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert!(stored.id.is_some());

        // get without id → returns the one we set.
        let fetched = svc.get_push_config("t-1", None).await.unwrap().unwrap();
        assert_eq!(fetched.url, "https://example.com/hook");

        // get by id → same.
        let fetched_by_id = svc
            .get_push_config("t-1", stored.id.as_deref())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched_by_id.id, stored.id);

        // list → contains exactly one.
        let listed = svc.list_push_configs("t-1").await.unwrap();
        assert_eq!(listed.len(), 1);

        // Add a second config so list_push_configs has something to enumerate.
        svc.set_push_config(
            "t-1",
            PushNotificationConfig {
                id: Some("explicit".into()),
                url: "https://example.com/hook2".into(),
                token: None,
                authentication: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(svc.list_push_configs("t-1").await.unwrap().len(), 2);

        // Delete by id → 1 removed, 1 remains.
        assert_eq!(
            svc.delete_push_config("t-1", Some("explicit"))
                .await
                .unwrap(),
            1
        );
        assert_eq!(svc.list_push_configs("t-1").await.unwrap().len(), 1);

        // Delete all (config_id=None) → removes the rest.
        assert_eq!(svc.delete_push_config("t-1", None).await.unwrap(), 1);
        assert!(svc.list_push_configs("t-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn push_config_rejects_unknown_task() {
        let svc = InMemoryTaskService::new();
        let r = svc
            .set_push_config(
                "missing",
                PushNotificationConfig {
                    id: None,
                    url: "https://example.com".into(),
                    token: None,
                    authentication: None,
                },
            )
            .await
            .unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn get_task_respects_history_length() {
        let svc = InMemoryTaskService::new();
        svc.create_task(new_task("t-1")).await.unwrap();
        for i in 0..5 {
            svc.append_history("t-1", Message::user_text(format!("m{i}")))
                .await
                .unwrap();
        }
        let t = svc.get_task("t-1", Some(2)).await.unwrap().unwrap();
        assert_eq!(t.history.len(), 2);
        assert_eq!(t.history[0].text_concat(), "m3");
        assert_eq!(t.history[1].text_concat(), "m4");
    }
}
