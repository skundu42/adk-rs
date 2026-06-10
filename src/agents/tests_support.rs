//! Internal test helpers — a stub agent and a default `InvocationContext`.
//! Compiled only under `#[cfg(test)]`.

use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;

use crate::agents::base::BaseAgent;
use crate::core::{Event, EventActions, EventStream, InvocationContext, LlmResponse};
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

pub(crate) fn test_ctx() -> Arc<InvocationContext> {
    let mut ctx = crate::core::testing::test_invocation_context();
    ctx.user_content = Some(Content::user_text("hi"));
    Arc::new(ctx)
}
