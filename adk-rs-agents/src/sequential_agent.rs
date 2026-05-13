//! [`SequentialAgent`] — run sub-agents one after another, in order. Each
//! sub-agent sees the cumulative event history.

use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::StreamExt;

use adk_rs_core::{EventStream, InvocationContext};
use adk_rs_error::{Error, Result};

use crate::base::BaseAgent;

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
