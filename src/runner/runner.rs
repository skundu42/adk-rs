//! [`Runner`] — top-level orchestrator. Owns the service trio, the agent
//! tree, and the plugin manager; produces an event stream per call.

use std::collections::HashMap;
use std::sync::Arc;

use async_stream::try_stream;
use futures::StreamExt;
use parking_lot::Mutex;
use tracing::{error, instrument};

use crate::agents::BaseAgent;
use crate::core::{
    ArtifactService, CancellationToken, CredentialService, Event, EventStream, GetSessionConfig,
    InvocationContext, InvocationOrigin, MemoryService, RunConfig, Session, SessionService,
};
use crate::error::{Error, Result};
use crate::genai_types::Content;

use crate::runner::plugin::PluginManager;

/// Handle for an in-flight invocation. Lets the caller observe the
/// generated `invocation_id`, request cancellation, and consume the agent's
/// event stream.
pub struct RunningInvocation {
    /// Server-assigned invocation id. Stable for the lifetime of the run.
    pub invocation_id: String,
    /// Shared cancellation flag. Cloned from
    /// [`InvocationContext::cancellation`] so flipping it here propagates
    /// to the running agent.
    pub cancellation: CancellationToken,
    /// The agent's event stream.
    pub events: EventStream<'static>,
}

impl std::fmt::Debug for RunningInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningInvocation")
            .field("invocation_id", &self.invocation_id)
            .field("cancelled", &self.cancellation.is_cancelled())
            .finish_non_exhaustive()
    }
}

/// Drops an entry from `Runner::active` when the event stream ends. Lives
/// inside the `try_stream!` block so the deregistration happens regardless
/// of how the stream terminates (completion, early `?` return, or caller-
/// side drop of the boxed stream).
struct ActiveGuard {
    active: Arc<Mutex<HashMap<String, CancellationToken>>>,
    invocation_id: String,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.active.lock().remove(&self.invocation_id);
    }
}

/// Top-level orchestrator.
pub struct Runner {
    app_name: String,
    agent: Arc<dyn BaseAgent>,
    session_service: Arc<dyn SessionService>,
    artifact_service: Option<Arc<dyn ArtifactService>>,
    memory_service: Option<Arc<dyn MemoryService>>,
    credential_service: Option<Arc<dyn CredentialService>>,
    plugins: Arc<PluginManager>,
    auto_create_session: bool,
    /// In-flight invocations, keyed by `invocation_id`. Entries are added
    /// at the top of [`Self::start`] and removed when the corresponding
    /// event stream completes. [`Self::cancel`] flips an entry's token
    /// without touching the map so the stream observes cancellation and
    /// drops the entry itself.
    active: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("app_name", &self.app_name)
            .field("agent", &self.agent.name())
            .field("auto_create_session", &self.auto_create_session)
            .finish_non_exhaustive()
    }
}

impl Runner {
    /// Start building.
    pub fn builder() -> RunnerBuilder {
        RunnerBuilder::default()
    }

    /// App name.
    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    /// Root agent.
    pub fn agent(&self) -> &Arc<dyn BaseAgent> {
        &self.agent
    }

    /// Session service.
    pub fn session_service(&self) -> &Arc<dyn SessionService> {
        &self.session_service
    }

    /// Run a single turn against `user_text`. Returns a stream of events.
    /// If `session_id` is None and `auto_create_session` is true, a new
    /// session is created.
    #[instrument(skip(self, user_text), fields(app=%self.app_name, agent=%self.agent.name()))]
    pub async fn run(
        &self,
        user_id: &str,
        session_id: Option<&str>,
        user_text: &str,
    ) -> Result<EventStream<'static>> {
        let run_cfg = RunConfig::default();
        self.run_with(user_id, session_id, Content::user_text(user_text), run_cfg)
            .await
    }

    /// Run with a typed [`Content`] and explicit [`RunConfig`]. Returns
    /// just the event stream — convenience wrapper around [`Self::start`].
    pub async fn run_with(
        &self,
        user_id: &str,
        session_id: Option<&str>,
        user_content: Content,
        run_config: RunConfig,
    ) -> Result<EventStream<'static>> {
        let handle = self
            .start(user_id, session_id, user_content, run_config)
            .await?;
        Ok(handle.events)
    }

    /// Request cancellation of an in-flight invocation by id. Returns
    /// `true` if a matching invocation was found and its token flipped, or
    /// `false` if the id is unknown (already finished or never started).
    /// Cancellation is cooperative — agents check the flag between LLM
    /// calls and tool dispatches and exit cleanly. Tools currently in
    /// flight will run to completion, but the agent stream won't issue
    /// further turns.
    pub fn cancel(&self, invocation_id: &str) -> bool {
        let guard = self.active.lock();
        if let Some(tok) = guard.get(invocation_id) {
            tok.cancel();
            true
        } else {
            false
        }
    }

    /// True if an invocation with this id is currently registered as
    /// in-flight.
    #[must_use]
    pub fn is_active(&self, invocation_id: &str) -> bool {
        self.active.lock().contains_key(invocation_id)
    }

    /// Start an invocation and return a [`RunningInvocation`] handle.
    /// Callers that need the `invocation_id` (e.g. to wire up
    /// [`Self::cancel`]) should use this in preference to [`Self::run_with`].
    pub async fn start(
        &self,
        user_id: &str,
        session_id: Option<&str>,
        user_content: Content,
        run_config: RunConfig,
    ) -> Result<RunningInvocation> {
        let session = self
            .load_or_create_session(user_id, session_id, None)
            .await?;
        let invocation_id = InvocationContext::new_id();
        let cancellation = CancellationToken::new();
        self.active
            .lock()
            .insert(invocation_id.clone(), cancellation.clone());
        let invocation = Arc::new(InvocationContext {
            app_name: self.app_name.clone(),
            user_id: user_id.to_string(),
            invocation_id: invocation_id.clone(),
            session: Arc::new(Mutex::new(session.clone())),
            session_service: self.session_service.clone(),
            artifact_service: self.artifact_service.clone(),
            memory_service: self.memory_service.clone(),
            credential_service: self.credential_service.clone(),
            run_config,
            origin: InvocationOrigin::Api,
            user_content: Some(user_content.clone()),
            llm_call_count: Arc::new(Mutex::new(0)),
            cancellation: cancellation.clone(),
            attributes: Arc::new(Mutex::new(HashMap::new())),
        });

        // Persist the user event before launching the agent.
        let mut user_ev = Event::new(
            "user",
            crate::core::LlmResponse {
                content: Some(user_content),
                ..Default::default()
            },
        );
        user_ev.invocation_id = invocation.invocation_id.clone();

        #[cfg(feature = "auth")]
        {
            let outcome = crate::auth::AuthPreprocessor::new()
                .process_event(
                    &user_ev,
                    &self.app_name,
                    user_id,
                    self.credential_service.clone(),
                )
                .await?;
            let mut attrs = invocation.attributes.lock();
            attrs.insert(
                "auth.resumed_tool_call_ids".into(),
                serde_json::to_value(outcome.resumed_tool_call_ids)?,
            );
            attrs.insert(
                "auth.resumed_toolset_ids".into(),
                serde_json::to_value(outcome.resumed_toolset_ids)?,
            );
        }

        // Atomic read-modify-write through the live Mutex — prevents
        // concurrent writers from being silently overwritten.
        self.session_service
            .append_event_locked(&invocation.session, user_ev.clone())
            .await?;

        self.plugins.before_run(&invocation).await?;

        let agent = self.agent.clone();
        let inv = invocation.clone();
        let svc = self.session_service.clone();
        let plugins = self.plugins.clone();
        let active = self.active.clone();
        let invocation_id_for_dedup = invocation_id.clone();
        let stream = try_stream! {
            // Deregister from `active` no matter how this stream ends —
            // success, error, cancel, or caller-side drop. We can't use
            // `Drop` on the stream itself (try_stream! captures by move),
            // so use a guard wrapper.
            let _guard = ActiveGuard {
                active: active.clone(),
                invocation_id: invocation_id_for_dedup.clone(),
            };
            let agent_stream = agent.run(inv.clone()).await;
            match agent_stream {
                Ok(mut s) => {
                    while let Some(ev) = s.next().await {
                        match ev {
                            Ok(ev) => {
                                // Persist non-partial, non-user events via the service.
                                // The agent already pushes to session.events; service mirrors.
                                if ev.partial != Some(true) && ev.author != "user" {
                                    // Atomic: hold the live lock for the
                                    // state-delta apply + event push. The
                                    // agent already pushed this event to the
                                    // in-memory session — remove the matching
                                    // id under the same lock so we don't
                                    // duplicate it. Use id-search (not
                                    // last-only) to remain correct when
                                    // parallel sub-agents interleave pushes.
                                    {
                                        let mut sess = inv.session.lock();
                                        if let Some(pos) =
                                            sess.events.iter().rposition(|e| e.id == ev.id)
                                        {
                                            sess.events.remove(pos);
                                        }
                                    }
                                    if let Err(e) = svc
                                        .append_event_locked(&inv.session, ev.clone())
                                        .await
                                    {
                                        if let Err(after_err) =
                                            plugins.after_run(&inv, Some(&e)).await
                                        {
                                            error!("plugin after_run failed: {after_err}");
                                        }
                                        Err(e)?;
                                    }
                                }
                                if let Err(e) = plugins.on_event(&inv, &ev).await {
                                    if let Err(after_err) = plugins.after_run(&inv, Some(&e)).await {
                                        error!("plugin after_run failed: {after_err}");
                                    }
                                    Err(e)?;
                                }
                                yield ev;
                            }
                            Err(e) => {
                                if let Err(after_err) = plugins.after_run(&inv, Some(&e)).await {
                                    error!("plugin after_run failed: {after_err}");
                                }
                                Err(e)?;
                            }
                        }
                    }
                }
                Err(e) => {
                    if let Err(after_err) = plugins.after_run(&inv, Some(&e)).await {
                        error!("plugin after_run failed: {after_err}");
                    }
                    Err(e)?;
                }
            }
            plugins.after_run(&inv, None).await?;
        };
        Ok(RunningInvocation {
            invocation_id,
            cancellation,
            events: Box::pin(stream),
        })
    }

    async fn load_or_create_session(
        &self,
        user_id: &str,
        session_id: Option<&str>,
        state: Option<crate::core::State>,
    ) -> Result<Session> {
        match session_id {
            Some(sid) => {
                if let Some(s) = self
                    .session_service
                    .get_session(&self.app_name, user_id, sid, GetSessionConfig::default())
                    .await?
                {
                    return Ok(s);
                }
                if self.auto_create_session {
                    self.session_service
                        .create_session(&self.app_name, user_id, state, Some(sid))
                        .await
                } else {
                    Err(Error::not_found(format!("session {sid}")))
                }
            }
            None => {
                self.session_service
                    .create_session(&self.app_name, user_id, state, None)
                    .await
            }
        }
    }
}

/// Builder for [`Runner`].
#[derive(Default)]
pub struct RunnerBuilder {
    app_name: Option<String>,
    agent: Option<Arc<dyn BaseAgent>>,
    session_service: Option<Arc<dyn SessionService>>,
    artifact_service: Option<Arc<dyn ArtifactService>>,
    memory_service: Option<Arc<dyn MemoryService>>,
    credential_service: Option<Arc<dyn CredentialService>>,
    plugins: PluginManager,
    auto_create_session: bool,
}

impl std::fmt::Debug for RunnerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerBuilder").finish_non_exhaustive()
    }
}

impl RunnerBuilder {
    /// App name (required).
    #[must_use]
    pub fn app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = Some(name.into());
        self
    }
    /// Root agent (required).
    #[must_use]
    pub fn agent(mut self, agent: Arc<dyn BaseAgent>) -> Self {
        self.agent = Some(agent);
        self
    }
    /// Session service (required).
    #[must_use]
    pub fn session_service(mut self, s: Arc<dyn SessionService>) -> Self {
        self.session_service = Some(s);
        self
    }
    /// Artifact service.
    #[must_use]
    pub fn artifact_service(mut self, s: Arc<dyn ArtifactService>) -> Self {
        self.artifact_service = Some(s);
        self
    }
    /// Memory service.
    #[must_use]
    pub fn memory_service(mut self, s: Arc<dyn MemoryService>) -> Self {
        self.memory_service = Some(s);
        self
    }
    /// Credential service.
    #[must_use]
    pub fn credential_service(mut self, s: Arc<dyn CredentialService>) -> Self {
        self.credential_service = Some(s);
        self
    }
    /// Auto-create session on missing id.
    #[must_use]
    pub fn auto_create_session(mut self, yes: bool) -> Self {
        self.auto_create_session = yes;
        self
    }

    /// Register a plugin.
    pub async fn plugin(mut self, p: Arc<dyn crate::runner::plugin::BasePlugin>) -> Result<Self> {
        self.plugins.register(p).await?;
        Ok(self)
    }

    /// Build.
    pub fn build(self) -> Result<Runner> {
        Ok(Runner {
            app_name: self
                .app_name
                .ok_or_else(|| Error::config("Runner requires app_name"))?,
            agent: self
                .agent
                .ok_or_else(|| Error::config("Runner requires agent"))?,
            session_service: self
                .session_service
                .ok_or_else(|| Error::config("Runner requires session_service"))?,
            artifact_service: self.artifact_service,
            memory_service: self.memory_service,
            credential_service: self.credential_service,
            plugins: Arc::new(self.plugins),
            auto_create_session: self.auto_create_session,
            active: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::LlmAgent;
    #[cfg(feature = "auth")]
    use crate::core::DynTool;
    use crate::core::Model;
    use crate::core::testing::MockModel;
    use crate::genai_types::Content;
    use crate::runner::plugin::BasePlugin;
    use crate::services::mem::InMemorySessionService;
    use async_trait::async_trait;
    #[cfg(feature = "auth")]
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn runner_runs_simple_turn() {
        let m = Arc::new(MockModel::new("mock-1"));
        m.push_text("hi back");
        let agent = Arc::new(
            LlmAgent::builder("greeter")
                .model(m.clone() as Arc<dyn Model>)
                .instruction("Greet")
                .build()
                .unwrap(),
        );
        let runner = Runner::builder()
            .app_name("hello")
            .agent(agent)
            .session_service(Arc::new(InMemorySessionService::new()))
            .build()
            .unwrap();
        let mut s = runner.run("u", None, "hello").await.unwrap();
        let mut events = Vec::new();
        while let Some(e) = s.next().await {
            events.push(e.unwrap());
        }
        assert!(!events.is_empty());
        let last = events.last().unwrap();
        assert_eq!(
            last.response.content.as_ref().unwrap().text_concat(),
            "hi back"
        );
    }

    #[tokio::test]
    async fn runner_records_user_event_in_session() {
        let m = Arc::new(MockModel::new("mock-1"));
        m.push_text("yo");
        let agent = Arc::new(
            LlmAgent::builder("a")
                .model(m.clone() as Arc<dyn Model>)
                .instruction("x")
                .build()
                .unwrap(),
        );
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let runner = Runner::builder()
            .app_name("hello")
            .agent(agent)
            .session_service(svc.clone())
            .build()
            .unwrap();
        let s = runner.run("u", None, "hi").await.unwrap();
        // Drain.
        s.collect::<Vec<_>>().await;
        let list = svc.list_sessions("hello", "u").await.unwrap();
        assert_eq!(list.sessions.len(), 1);
        let sess = svc
            .get_session(
                "hello",
                "u",
                &list.sessions[0].id,
                GetSessionConfig::default(),
            )
            .await
            .unwrap()
            .unwrap();
        // User event + model event minimum.
        assert!(sess.events.len() >= 2);
        assert_eq!(sess.events[0].author, "user");
    }

    #[tokio::test]
    async fn runner_does_not_duplicate_current_user_content() {
        let m = Arc::new(MockModel::new("mock-1"));
        m.push_text("ok");
        let agent = Arc::new(
            LlmAgent::builder("a")
                .model(m.clone() as Arc<dyn Model>)
                .build()
                .unwrap(),
        );
        let runner = Runner::builder()
            .app_name("hello")
            .agent(agent)
            .session_service(Arc::new(InMemorySessionService::new()))
            .build()
            .unwrap();
        let mut s = runner.run("u", None, "hi").await.unwrap();
        while let Some(e) = s.next().await {
            e.unwrap();
        }
        let reqs = m.captured_requests();
        assert_eq!(reqs.len(), 1);
        let user_count = reqs[0]
            .contents
            .iter()
            .filter(|c| *c == &Content::user_text("hi"))
            .count();
        assert_eq!(user_count, 1);
    }

    #[tokio::test]
    async fn start_returns_running_invocation_handle_with_stable_id() {
        let m = Arc::new(MockModel::new("mock-1"));
        m.push_text("hi");
        let agent = Arc::new(
            LlmAgent::builder("a")
                .model(m.clone() as Arc<dyn Model>)
                .build()
                .unwrap(),
        );
        let runner = Runner::builder()
            .app_name("hello")
            .agent(agent)
            .session_service(Arc::new(InMemorySessionService::new()))
            .build()
            .unwrap();
        let handle = runner
            .start("u", None, Content::user_text("hi"), RunConfig::default())
            .await
            .unwrap();
        let id = handle.invocation_id.clone();
        assert!(runner.is_active(&id));
        // Drain the stream so the ActiveGuard fires.
        handle
            .events
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(!runner.is_active(&id));
    }

    #[tokio::test]
    async fn cancel_unknown_id_returns_false() {
        let m = Arc::new(MockModel::new("mock-1"));
        m.push_text("ok");
        let agent = Arc::new(
            LlmAgent::builder("a")
                .model(m as Arc<dyn Model>)
                .build()
                .unwrap(),
        );
        let runner = Runner::builder()
            .app_name("hello")
            .agent(agent)
            .session_service(Arc::new(InMemorySessionService::new()))
            .build()
            .unwrap();
        assert!(!runner.cancel("nope"));
    }

    #[tokio::test]
    async fn cancel_before_polling_stream_emits_cancelled_first() {
        // Set up a model with several turns queued. If the agent ran to
        // completion it would emit "first", "second", etc. We cancel
        // before polling the stream — the agent's loop checks the
        // cancellation flag at the top of its first iteration and exits
        // with a CANCELLED event before issuing any LLM call.
        let m = Arc::new(MockModel::new("mock-1"));
        m.push_text("first");
        m.push_text("second");
        let agent = Arc::new(
            LlmAgent::builder("a")
                .model(m.clone() as Arc<dyn Model>)
                .build()
                .unwrap(),
        );
        let runner = Arc::new(
            Runner::builder()
                .app_name("hello")
                .agent(agent)
                .session_service(Arc::new(InMemorySessionService::new()))
                .build()
                .unwrap(),
        );

        let handle = runner
            .start("u", None, Content::user_text("go"), RunConfig::default())
            .await
            .unwrap();
        let inv_id = handle.invocation_id.clone();
        // Cancel via the public Runner::cancel API — same path A2A uses.
        assert!(runner.cancel(&inv_id));
        let events = handle
            .events
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .unwrap();

        // The stream must contain a CANCELLED event and *no* successful
        // model text events.
        assert!(
            events
                .iter()
                .any(|e| e.response.error_code.as_deref() == Some("CANCELLED")),
            "expected a CANCELLED event; got {:?}",
            events
                .iter()
                .map(|e| (
                    e.author.clone(),
                    e.response.error_code.clone(),
                    e.response.content.as_ref().map(|c| c.text_concat())
                ))
                .collect::<Vec<_>>()
        );
        assert!(
            !events.iter().any(|e| {
                e.response
                    .content
                    .as_ref()
                    .map(|c| c.text_concat() == "first" || c.text_concat() == "second")
                    .unwrap_or(false)
            }),
            "agent emitted model text after cancellation"
        );
        // Active map cleaned up by ActiveGuard.
        assert!(!runner.is_active(&inv_id));
    }

    #[derive(Debug)]
    struct FailingEventPlugin {
        after_errors: AtomicUsize,
    }

    #[async_trait]
    impl BasePlugin for FailingEventPlugin {
        async fn on_event(&self, _: &InvocationContext, _: &Event) -> Result<()> {
            Err(Error::other("plugin event failed"))
        }

        async fn after_run(&self, _: &InvocationContext, err: Option<&Error>) -> Result<()> {
            if err.is_some() {
                self.after_errors.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn runner_propagates_plugin_event_errors_and_reports_after_run_error() {
        let m = Arc::new(MockModel::new("mock-1"));
        m.push_text("ok");
        let agent = Arc::new(
            LlmAgent::builder("a")
                .model(m.clone() as Arc<dyn Model>)
                .build()
                .unwrap(),
        );
        let plugin = Arc::new(FailingEventPlugin {
            after_errors: AtomicUsize::new(0),
        });
        let builder = Runner::builder()
            .app_name("hello")
            .agent(agent)
            .session_service(Arc::new(InMemorySessionService::new()))
            .plugin(plugin.clone())
            .await
            .unwrap();
        let runner = builder.build().unwrap();
        let mut s = runner.run("u", None, "hi").await.unwrap();
        let err = s.next().await.unwrap().unwrap_err();
        assert!(err.to_string().contains("plugin event failed"));
        assert_eq!(plugin.after_errors.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "auth")]
    #[derive(Debug)]
    struct AuthEchoTool {
        cfg: crate::auth::AuthConfig,
    }

    #[cfg(feature = "auth")]
    #[async_trait]
    impl DynTool for AuthEchoTool {
        fn name(&self) -> &str {
            "needs_auth"
        }

        fn description(&self) -> &str {
            "returns the resolved OAuth access token"
        }

        fn auth_config(&self) -> Option<&crate::auth::AuthConfig> {
            Some(&self.cfg)
        }

        fn declaration(&self) -> Option<crate::genai_types::FunctionDeclaration> {
            Some(crate::genai_types::FunctionDeclaration::new(
                self.name(),
                self.description(),
            ))
        }

        async fn run(
            &self,
            _: serde_json::Value,
            ctx: &mut crate::core::ToolContext,
        ) -> Result<serde_json::Value> {
            let token = ctx
                .auth_credential
                .as_ref()
                .and_then(|c| c.oauth2.as_ref())
                .and_then(|o| o.access_token.as_deref())
                .ok_or_else(|| Error::other("missing resolved token"))?;
            Ok(json!({ "token": token }))
        }
    }

    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn runner_absorbs_auth_response_and_replays_deferred_tool_call() {
        use crate::auth::{AuthConfig, AuthCredential, InMemoryCredentialService, OAuth2Auth};
        use crate::auth::{AuthScheme, OAuthFlow, OAuthFlows};
        use crate::genai_types::{FunctionCall, FunctionResponse, Part, Role};

        let cfg = AuthConfig::new(AuthScheme::OAuth2 {
            flows: OAuthFlows {
                authorization_code: Some(OAuthFlow {
                    authorization_url: Some("https://provider/authorize".into()),
                    token_url: "https://provider/token".into(),
                    refresh_url: None,
                    scopes: Default::default(),
                }),
                ..OAuthFlows::default()
            },
            description: None,
        })
        .with_raw(AuthCredential::oauth2(OAuth2Auth {
            client_id: "client".into(),
            client_secret: Some("secret".into()),
            ..OAuth2Auth::default()
        }))
        .with_key("oauth-key");

        let m = Arc::new(MockModel::new("mock-1"));
        m.push_response(crate::core::LlmResponse {
            content: Some(Content {
                role: Role::Model,
                parts: vec![Part::FunctionCall(
                    FunctionCall::new("needs_auth", json!({})).with_id("call-1"),
                )],
            }),
            ..Default::default()
        });
        m.push_text("done");

        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let runner = Runner::builder()
            .app_name("hello")
            .agent(Arc::new(
                LlmAgent::builder("a")
                    .model(m.clone() as Arc<dyn Model>)
                    .tool(Arc::new(AuthEchoTool { cfg: cfg.clone() }))
                    .build()
                    .unwrap(),
            ))
            .session_service(svc.clone())
            .credential_service(Arc::new(InMemoryCredentialService::new()))
            .build()
            .unwrap();

        let mut first = runner.run("u", None, "start").await.unwrap();
        let first_events: Vec<Event> = first
            .by_ref()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let pending = first_events
            .iter()
            .flat_map(Event::function_responses)
            .find(|fr| fr.name == crate::auth::REQUEST_CREDENTIAL_FUNCTION_NAME)
            .expect("pending auth response");
        assert_eq!(pending.id.as_deref(), Some("call-1"));

        let session_id = svc.list_sessions("hello", "u").await.unwrap().sessions[0]
            .id
            .clone();
        let mut returned_cfg = cfg.clone();
        returned_cfg.exchanged_auth_credential = Some(AuthCredential::oauth2(OAuth2Auth {
            client_id: "client".into(),
            access_token: Some("TOKEN".into()),
            ..OAuth2Auth::default()
        }));
        let auth_content = Content {
            role: Role::User,
            parts: vec![Part::FunctionResponse(FunctionResponse {
                id: Some("call-1".into()),
                name: crate::auth::REQUEST_CREDENTIAL_FUNCTION_NAME.into(),
                response: serde_json::to_value(returned_cfg).unwrap(),
                will_continue: None,
                scheduling: None,
            })],
        };

        let mut second = runner
            .run_with("u", Some(&session_id), auth_content, RunConfig::default())
            .await
            .unwrap();
        let second_events = second
            .by_ref()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(second_events.iter().any(|e| {
            e.function_responses().iter().any(|fr| {
                fr.name == "needs_auth" && fr.response["token"] == serde_json::json!("TOKEN")
            })
        }));
    }
}
