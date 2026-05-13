//! [`SequentialAgent`] — run sub-agents one after another, in order. Each
//! sub-agent sees the cumulative event history.

use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::StreamExt;

use crate::core::{EventStream, InvocationContext};
use crate::error::{Error, Result};

use crate::agents::base::BaseAgent;

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
            for sub in &me.sub_agents {
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
