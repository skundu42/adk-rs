//! Service traits: session, artifact, memory, credential.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::genai_types::Part;

use crate::core::artifact::ArtifactKey;
use crate::core::event::Event;
use crate::core::memory::SearchMemoryResponse;
use crate::core::session::{Session, SessionId, SessionMeta};
use crate::core::state::State;

/// Optional filters on `get_session`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GetSessionConfig {
    /// Limit to the most recent N events.
    pub num_recent_events: Option<usize>,
    /// Only include events with timestamp ≥ this value.
    pub after_timestamp: Option<f64>,
}

/// Convenience alias used in older Python ADK versions.
pub type SessionsMeta = ListSessionsResponse;

/// Response shape for `list_sessions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListSessionsResponse {
    /// Sessions owned by the queried `(app, user)`.
    pub sessions: Vec<SessionMeta>,
}

/// Service for creating, fetching, listing, and updating sessions.
#[async_trait]
pub trait SessionService: Send + Sync + std::fmt::Debug + 'static {
    /// Create a new session. If `id` is None, generate one.
    async fn create_session(
        &self,
        app_name: &str,
        user_id: &str,
        state: Option<State>,
        id: Option<&str>,
    ) -> Result<Session>;

    /// Fetch a session.
    async fn get_session(
        &self,
        app_name: &str,
        user_id: &str,
        session_id: &str,
        config: GetSessionConfig,
    ) -> Result<Option<Session>>;

    /// List sessions for `(app, user)`.
    async fn list_sessions(&self, app_name: &str, user_id: &str) -> Result<ListSessionsResponse>;

    /// Delete a session.
    async fn delete_session(&self, app_name: &str, user_id: &str, session_id: &str) -> Result<()>;

    /// Append an event to the session and persist.
    ///
    /// The default impl handles state-delta application (including
    /// `temp:` trimming) and updates `last_update_time`. Backends with
    /// transactional semantics may override.
    async fn append_event(&self, session: &mut Session, mut event: Event) -> Result<Event> {
        if event.partial == Some(true) {
            return Ok(event);
        }
        // Apply temp state to in-memory session BEFORE trimming so subsequent
        // agents in the same invocation can still read it, then trim the
        // delta so it isn't persisted.
        for (k, v) in &event.actions.state_delta {
            if crate::core::state::StateScope::of(k) == crate::core::state::StateScope::Temp {
                session.state.set(k.clone(), v.clone());
            }
        }
        event.actions.state_delta = State::trim_temp_keys(&event.actions.state_delta);
        session.state.apply(&event.actions.state_delta);
        session.last_update_time = crate::core::session::now_secs();
        session.events.push(event.clone());
        Ok(event)
    }

    /// Optional flush hook for buffering backends.
    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}

/// Service for storing and fetching versioned artifacts.
#[async_trait]
pub trait ArtifactService: Send + Sync + std::fmt::Debug + 'static {
    /// Save an artifact; returns the new version (1-indexed).
    async fn save_artifact(&self, key: ArtifactKey, part: Part) -> Result<u64>;

    /// Load an artifact (latest version if `version` is None).
    async fn load_artifact(&self, key: ArtifactKey, version: Option<u64>) -> Result<Option<Part>>;

    /// List filenames in a session.
    async fn list_artifact_keys(
        &self,
        app_name: &str,
        user_id: &str,
        session_id: &str,
    ) -> Result<Vec<String>>;

    /// Delete all versions of an artifact.
    async fn delete_artifact(&self, key: ArtifactKey) -> Result<()>;

    /// List all versions of an artifact.
    async fn list_versions(&self, key: ArtifactKey) -> Result<Vec<u64>>;
}

/// Service for ingesting completed sessions into long-term memory and
/// querying it.
#[async_trait]
pub trait MemoryService: Send + Sync + std::fmt::Debug + 'static {
    /// Index a session's events into memory.
    async fn add_session_to_memory(&self, session: &Session) -> Result<()>;

    /// Search memory for entries matching `query`.
    async fn search_memory(
        &self,
        app_name: &str,
        user_id: &str,
        query: &str,
    ) -> Result<SearchMemoryResponse>;
}

/// Service for storing and refreshing tool credentials.
#[async_trait]
pub trait CredentialService: Send + Sync + std::fmt::Debug + 'static {
    /// Load a credential value by key.
    async fn load(&self, app_name: &str, user_id: &str, key: &str) -> Result<Option<String>>;

    /// Store a credential value by key.
    async fn save(&self, app_name: &str, user_id: &str, key: &str, value: &str) -> Result<()>;
}

/// Helper used by simpler `SessionService` impls.
pub fn new_session_id() -> SessionId {
    uuid::Uuid::new_v4().to_string()
}
