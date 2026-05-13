//! [`LoopAgent`] — repeatedly run a sub-agent until it escalates or a max
//! iteration count is reached.

use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::StreamExt;

use adk_rs_core::{EventStream, InvocationContext};
use adk_rs_error::{Error, Result};

use crate::base::BaseAgent;

/// Repeatedly run a sub-agent until escalation or the iteration cap.
#[derive(Debug)]
pub struct LoopAgent {
    name: String,
    description: String,
    sub_agents: Vec<Arc<dyn BaseAgent>>,
    max_iterations: u32,
}

impl LoopAgent {
    /// Construct.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        sub_agents: Vec<Arc<dyn BaseAgent>>,
        max_iterations: u32,
    ) -> Result<Self> {
        if sub_agents.is_empty() {
            return Err(Error::config("LoopAgent requires at least one sub_agent"));
        }
        if max_iterations == 0 {
            return Err(Error::config("LoopAgent.max_iterations must be > 0"));
        }
        Ok(Self {
            name: name.into(),
            description: description.into(),
            sub_agents,
            max_iterations,
        })
    }
}

#[async_trait]
impl BaseAgent for LoopAgent {
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
            'outer: for _i in 0..me.max_iterations {
                for sub in &me.sub_agents {
                    let mut s = Box::pin(sub.clone().run(ctx.clone()).await?);
                    while let Some(ev) = s.next().await {
                        let ev = ev?;
                        let escalate = ev.actions.escalate == Some(true);
                        yield ev;
                        if escalate {
                            break 'outer;
                        }
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}
