//! Invocation and tool execution contexts.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use parking_lot::Mutex;
use serde_json::Value;
use uuid::Uuid;

use crate::genai_types::Content;

use crate::core::run_config::RunConfig;
use crate::core::services::{ArtifactService, CredentialService, MemoryService, SessionService};
use crate::core::session::Session;
use crate::core::state::StateDelta;

/// Where the invocation came from. Mostly informational; the runner uses this
/// to decide whether to auto-create a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InvocationOrigin {
    /// Direct API call (default).
    #[default]
    Api,
    /// CLI / REPL.
    Cli,
    /// Web server / browser.
    Web,
}

/// Per-invocation context carried through the agent run.
#[derive(Clone)]
pub struct InvocationContext {
    /// App name.
    pub app_name: String,
    /// User id.
    pub user_id: String,
    /// Invocation id (unique per `runner.run` call).
    pub invocation_id: String,
    /// The session being mutated. Wrapped in [`Arc<Mutex>`] so the runner
    /// and tool callbacks can both mutate state safely.
    pub session: Arc<Mutex<Session>>,
    /// Active session service.
    pub session_service: Arc<dyn SessionService>,
    /// Optional artifact service.
    pub artifact_service: Option<Arc<dyn ArtifactService>>,
    /// Optional memory service.
    pub memory_service: Option<Arc<dyn MemoryService>>,
    /// Optional credential service.
    pub credential_service: Option<Arc<dyn CredentialService>>,
    /// Per-invocation config.
    pub run_config: RunConfig,
    /// Origin of the invocation (CLI, web, API).
    pub origin: InvocationOrigin,
    /// User content for this invocation, if any.
    pub user_content: Option<Content>,
    /// Counter of LLM calls performed (capped by `RunConfig::max_llm_calls`).
    pub llm_call_count: Arc<Mutex<u32>>,
    /// Free-form attribute bag for plugins / agent-specific bookkeeping.
    pub attributes: Arc<Mutex<HashMap<String, Value>>>,
}

impl std::fmt::Debug for InvocationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationContext")
            .field("app_name", &self.app_name)
            .field("user_id", &self.user_id)
            .field("invocation_id", &self.invocation_id)
            .field("origin", &self.origin)
            .finish()
    }
}

impl InvocationContext {
    /// Generate a fresh invocation id.
    #[must_use]
    pub fn new_id() -> String {
        format!("inv-{}", Uuid::new_v4())
    }

    /// Increment the LLM call counter. Returns `Err` if the cap was already
    /// reached.
    pub fn check_and_inc_llm_call(&self) -> crate::error::Result<()> {
        let mut n = self.llm_call_count.lock();
        if let Some(cap) = self.run_config.max_llm_calls {
            if *n >= cap {
                return Err(crate::error::Error::config(format!(
                    "max_llm_calls={cap} reached"
                )));
            }
        }
        *n += 1;
        Ok(())
    }
}

/// Per-call context passed to a tool's `run` method.
pub struct ToolContext {
    /// Underlying invocation context.
    pub invocation: Arc<InvocationContext>,
    /// Function-call id (matches `FunctionCall::id`).
    pub function_call_id: Option<String>,
    /// State delta accumulator that the tool can mutate.
    pub state_delta: StateDelta,
    /// Artifact-version delta accumulator.
    pub artifact_delta: IndexMap<String, u64>,
    /// If set on return, the runner skips summarization of the response.
    pub skip_summarization: bool,
    /// If set on return, the runner transfers control to the named agent.
    pub transfer_to_agent: Option<String>,
    /// If set on return, the runner unwinds escalations.
    pub escalate: bool,
    /// If true, the tool returned a long-running operation handle.
    pub long_running: bool,
}

impl ToolContext {
    /// Construct.
    pub fn new(invocation: Arc<InvocationContext>) -> Self {
        Self {
            invocation,
            function_call_id: None,
            state_delta: StateDelta::new(),
            artifact_delta: IndexMap::new(),
            skip_summarization: false,
            transfer_to_agent: None,
            escalate: false,
            long_running: false,
        }
    }

    /// Set the function-call id.
    #[must_use]
    pub fn with_function_call_id(mut self, id: impl Into<String>) -> Self {
        self.function_call_id = Some(id.into());
        self
    }

    /// Save an artifact via the configured artifact service. Returns the
    /// new version. Errors if no artifact service is configured.
    pub async fn save_artifact(
        &mut self,
        filename: &str,
        part: crate::genai_types::Part,
    ) -> crate::error::Result<u64> {
        let svc = self
            .invocation
            .artifact_service
            .as_ref()
            .ok_or_else(|| crate::error::Error::config("no artifact service configured"))?;
        let key = crate::core::artifact::ArtifactKey::new(
            &self.invocation.app_name,
            &self.invocation.user_id,
            &self.invocation.session.lock().id,
            filename,
        );
        let v = svc.save_artifact(key, part).await?;
        self.artifact_delta.insert(filename.to_string(), v);
        Ok(v)
    }

    /// Load an artifact via the configured artifact service.
    pub async fn load_artifact(
        &self,
        filename: &str,
        version: Option<u64>,
    ) -> crate::error::Result<Option<crate::genai_types::Part>> {
        let svc = self
            .invocation
            .artifact_service
            .as_ref()
            .ok_or_else(|| crate::error::Error::config("no artifact service configured"))?;
        let key = crate::core::artifact::ArtifactKey::new(
            &self.invocation.app_name,
            &self.invocation.user_id,
            &self.invocation.session.lock().id,
            filename,
        );
        svc.load_artifact(key, version).await
    }
}
