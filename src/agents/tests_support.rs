//! Internal test helpers — a stub agent and a default `InvocationContext`.
//! Compiled only under `#[cfg(test)]`.

use std::collections::HashMap;
use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use parking_lot::Mutex;

use crate::agents::base::BaseAgent;
use crate::core::{
    Event, EventActions, EventStream, GetSessionConfig, InvocationContext, InvocationOrigin,
    ListSessionsResponse, LlmResponse, RunConfig, Session, SessionService, State,
};
use crate::error::Result;
use crate::genai_types::Content;

/// Agent that emits one event per provided text, then stops. If `escalate` is
/// true, the *last* emitted event has `actions.escalate = Some(true)`.
#[derive(Debug)]
pub(crate) struct StubAgent {
    name: String,
    texts: Vec<String>,
    escalate: bool,
}

pub(crate) fn stub_agent(name: &str, texts: &[&str], escalate: bool) -> Arc<dyn BaseAgent> {
    Arc::new(StubAgent {
        name: name.into(),
        texts: texts.iter().map(|s| (*s).to_string()).collect(),
        escalate,
    })
}

#[async_trait]
impl BaseAgent for StubAgent {
    fn name(&self) -> &str {
        &self.name
    }
    async fn run(self: Arc<Self>, _ctx: Arc<InvocationContext>) -> Result<EventStream<'static>> {
        let me = self.clone();
        let stream = try_stream! {
            let last = me.texts.len().saturating_sub(1);
            for (i, t) in me.texts.iter().enumerate() {
                let mut ev = Event::new(
                    me.name.clone(),
                    LlmResponse {
                        content: Some(Content::model_text(t)),
                        ..LlmResponse::default()
                    },
                );
                if me.escalate && i == last {
                    ev.actions = EventActions { escalate: Some(true), ..EventActions::default() };
                }
                yield ev;
            }
        };
        Ok(Box::pin(stream))
    }
}

#[derive(Debug)]
struct NoopSession;
#[async_trait]
impl SessionService for NoopSession {
    async fn create_session(
        &self,
        app: &str,
        user: &str,
        _state: Option<State>,
        id: Option<&str>,
    ) -> Result<Session> {
        Ok(Session::new(app, user, id.unwrap_or("s")))
    }
    async fn get_session(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: GetSessionConfig,
    ) -> Result<Option<Session>> {
        Ok(None)
    }
    async fn list_sessions(&self, _: &str, _: &str) -> Result<ListSessionsResponse> {
        Ok(ListSessionsResponse::default())
    }
    async fn delete_session(&self, _: &str, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
}

pub(crate) fn test_ctx() -> Arc<InvocationContext> {
    Arc::new(InvocationContext {
        app_name: "app".into(),
        user_id: "u".into(),
        invocation_id: "inv-1".into(),
        session: Arc::new(Mutex::new(Session::new("app", "u", "s"))),
        session_service: Arc::new(NoopSession),
        artifact_service: None,
        memory_service: None,
        credential_service: None,
        run_config: RunConfig::default(),
        origin: InvocationOrigin::Api,
        user_content: Some(Content::user_text("hi")),
        llm_call_count: Arc::new(Mutex::new(0)),
        attributes: Arc::new(Mutex::new(HashMap::new())),
    })
}
