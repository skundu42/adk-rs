//! [`Runner`] — top-level orchestrator. Owns the service trio, the agent
//! tree, and the plugin manager; produces an event stream per call.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_stream::try_stream;
use futures::StreamExt;
use parking_lot::Mutex;
use tracing::{error, instrument};

use crate::agents::BaseAgent;
use crate::core::{
    ArtifactService, CredentialService, Event, EventStream, GetSessionConfig, InvocationContext,
    InvocationOrigin, MemoryService, RunConfig, Session, SessionService,
};
use crate::error::{Error, Result};
use crate::genai_types::Content;

use crate::runner::plugin::PluginManager;

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

    /// Run with a typed [`Content`] and explicit [`RunConfig`].
    pub async fn run_with(
        &self,
        user_id: &str,
        session_id: Option<&str>,
        user_content: Content,
        run_config: RunConfig,
    ) -> Result<EventStream<'static>> {
        let session = self
            .load_or_create_session(user_id, session_id, None)
            .await?;
        let invocation = Arc::new(InvocationContext {
            app_name: self.app_name.clone(),
            user_id: user_id.to_string(),
            invocation_id: InvocationContext::new_id(),
            session: Arc::new(Mutex::new(session.clone())),
            session_service: self.session_service.clone(),
            artifact_service: self.artifact_service.clone(),
            memory_service: self.memory_service.clone(),
            credential_service: self.credential_service.clone(),
            run_config,
            origin: InvocationOrigin::Api,
            user_content: Some(user_content.clone()),
            llm_call_count: Arc::new(Mutex::new(0)),
            attributes: Arc::new(Mutex::new(HashMap::new())),
        });

        // Persist the user event before launching the agent.
        let user_ev = Event::new(
            "user",
            crate::core::LlmResponse {
                content: Some(user_content),
                ..Default::default()
            },
        );
        let mut sess_clone = invocation.session.lock().clone();
        self.session_service
            .append_event(&mut sess_clone, user_ev.clone())
            .await?;
        *invocation.session.lock() = sess_clone;

        self.plugins.before_run(&invocation).await?;

        let agent = self.agent.clone();
        let inv = invocation.clone();
        let svc = self.session_service.clone();
        let plugins = self.plugins.clone();
        let plugins_after = self.plugins.clone();
        let inv_after = invocation.clone();

        let stream = try_stream! {
            let mut s = Box::pin(agent.run(inv.clone()).await?);
            while let Some(ev) = s.next().await {
                let ev = ev?;
                // Persist non-partial, non-user events via the service.
                // The agent already pushes to session.events; service mirrors.
                if ev.partial != Some(true) && ev.author != "user" {
                    let mut sess_clone = inv.session.lock().clone();
                    // Note: the agent already appended this event in-memory;
                    // pop the last one so append_event doesn't duplicate it.
                    let already_in = sess_clone
                        .events
                        .last()
                        .map(|e| e.id == ev.id)
                        .unwrap_or(false);
                    if already_in {
                        sess_clone.events.pop();
                    }
                    svc.append_event(&mut sess_clone, ev.clone()).await?;
                    *inv.session.lock() = sess_clone;
                }
                let _ = plugins.on_event(&inv, &ev).await;
                yield ev;
            }
        };

        let wrapped = AfterRunStream {
            inner: Some(Box::pin(stream)),
            after: Some(Box::new(move || {
                let plugins = plugins_after.clone();
                let inv = inv_after.clone();
                Box::pin(async move {
                    if let Err(e) = plugins.after_run(&inv, None).await {
                        error!("plugin after_run failed: {e}");
                    }
                })
            })),
        };
        Ok(Box::pin(wrapped))
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

// -----------------------------------------------------------------------------
// AfterRunStream: wraps an EventStream and fires a finaliser when drained.

type AfterFn = Box<dyn FnOnce() -> futures::future::BoxFuture<'static, ()> + Send>;

struct AfterRunStream {
    inner: Option<EventStream<'static>>,
    after: Option<AfterFn>,
}

impl futures::Stream for AfterRunStream {
    type Item = Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(inner) = self.inner.as_mut() {
            let r = inner.as_mut().poll_next(cx);
            if let Poll::Ready(None) = r {
                self.inner = None;
                if let Some(f) = self.after.take() {
                    tokio::spawn(f());
                }
            }
            return r;
        }
        Poll::Ready(None)
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::LlmAgent;
    use crate::core::Model;
    use crate::core::testing::MockModel;
    use crate::services::mem::InMemorySessionService;

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
}
