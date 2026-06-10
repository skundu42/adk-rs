//! [`SequentialAgent`] — run sub-agents one after another, in order. Each
//! sub-agent sees the cumulative event history.

use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::StreamExt;

use crate::core::{Event, EventStream, InvocationContext, LlmResponse};
use crate::error::{Error, Result};

use crate::agents::base::BaseAgent;

/// True when [`crate::core::RunConfig::resumability`] enables resume.
pub(crate) fn is_resumable(ctx: &InvocationContext) -> bool {
    ctx.run_config
        .resumability
        .map(|r| r.is_resumable)
        .unwrap_or(false)
}

/// True when a descendant agent paused the invocation (long-running tool,
/// confirmation, or auth consent pending).
pub(crate) fn invocation_paused(ctx: &InvocationContext) -> bool {
    ctx.attributes
        .lock()
        .get("invocation.paused")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Latest checkpoint recorded by `author` within this invocation: how many
/// sub-agents have completed.
pub(crate) fn completed_sub_agents(ctx: &InvocationContext, author: &str) -> usize {
    let sess = ctx.session.lock();
    sess.events
        .iter()
        .rev()
        .find(|e| {
            e.invocation_id == ctx.invocation_id
                && e.author == author
                && e.actions.agent_state.is_some()
        })
        .and_then(|e| e.actions.agent_state.as_ref())
        .and_then(|s| s.get("completed_sub_agents"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize
}

/// Build a checkpoint event recording that `n` sub-agents completed.
pub(crate) fn checkpoint_event(author: &str, invocation_id: &str, n: usize) -> Event {
    let mut e = Event::new(author, LlmResponse::default());
    e.invocation_id = invocation_id.to_string();
    e.actions.agent_state = Some(serde_json::json!({ "completed_sub_agents": n }));
    e
}

/// Run sub-agents in declared order.
#[derive(Debug)]
pub struct SequentialAgent {
    name: String,
    description: String,
    sub_agents: Vec<Arc<dyn BaseAgent>>,
}

impl SequentialAgent {
    /// Construct.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        sub_agents: Vec<Arc<dyn BaseAgent>>,
    ) -> Result<Self> {
        if sub_agents.is_empty() {
            return Err(Error::config(
                "SequentialAgent requires at least one sub_agent",
            ));
        }
        Ok(Self {
            name: name.into(),
            description: description.into(),
            sub_agents,
        })
    }
}

#[async_trait]
impl BaseAgent for SequentialAgent {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn sub_agents(&self) -> &[Arc<dyn BaseAgent>] {
        &self.sub_agents
    }
    async fn run(self: Arc<Self>, ctx: Arc<InvocationContext>) -> Result<EventStream<'static>> {
        let me = self.clone();
        let stream = try_stream! {
            let resumable = is_resumable(&ctx);
            // On an in-place resume (same invocation id), skip sub-agents
            // that completed before the pause.
            let start_index = if resumable {
                completed_sub_agents(&ctx, &me.name)
            } else {
                0
            };
            for (i, sub) in me.sub_agents.iter().enumerate().skip(start_index) {
                if ctx.is_cancelled() {
                    return;
                }
                let mut s = Box::pin(sub.clone().run(ctx.clone()).await?);
                while let Some(ev) = s.next().await {
                    let ev = ev?;
                    // If a sub-agent escalates, stop the sequence.
                    let escalate = ev.actions.escalate == Some(true);
                    yield ev;
                    if escalate {
                        return;
                    }
                }
                // A paused sub-agent (HITL confirmation, auth consent,
                // long-running tool) suspends the whole pipeline; resume
                // re-enters at this index thanks to the checkpoint below.
                if invocation_paused(&ctx) {
                    return;
                }
                if resumable && i + 1 < me.sub_agents.len() {
                    yield checkpoint_event(&me.name, &ctx.invocation_id, i + 1);
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::tests_support::{stub_agent, test_ctx};

    #[tokio::test]
    async fn empty_sub_agents_rejected() {
        let err = SequentialAgent::new("seq", "d", vec![]).unwrap_err();
        assert!(err.to_string().contains("at least one sub_agent"));
    }

    #[tokio::test]
    async fn runs_sub_agents_in_declared_order() {
        let a = stub_agent("a", &["a-msg"], false);
        let b = stub_agent("b", &["b-msg"], false);
        let seq = Arc::new(SequentialAgent::new("seq", "", vec![a, b]).unwrap());
        let mut stream = seq.run(test_ctx()).await.unwrap();
        let mut authors = Vec::new();
        while let Some(ev) = stream.next().await {
            authors.push(ev.unwrap().author);
        }
        assert_eq!(authors, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn stops_after_escalate() {
        let a = stub_agent("a", &["a-msg"], true); // escalates
        let b = stub_agent("b", &["b-msg"], false);
        let seq = Arc::new(SequentialAgent::new("seq", "", vec![a, b]).unwrap());
        let mut stream = seq.run(test_ctx()).await.unwrap();
        let mut authors = Vec::new();
        while let Some(ev) = stream.next().await {
            authors.push(ev.unwrap().author);
        }
        assert_eq!(authors, vec!["a"], "b should not have run after escalate");
    }
}
