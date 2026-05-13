//! [`BaseAgent`] trait — the common shape of every agent.

use std::sync::Arc;

use async_trait::async_trait;

use crate::core::{EventStream, InvocationContext};
use crate::error::Result;

/// Common shape of every agent. Concrete agents in this crate (`LlmAgent`,
/// `SequentialAgent`, etc.) implement this; user-defined agents can too.
#[async_trait]
pub trait BaseAgent: Send + Sync + std::fmt::Debug + 'static {
    /// The agent's name. Must be a valid identifier and unique within an
    /// agent tree; the runner addresses agents by name for transfer.
    fn name(&self) -> &str;

    /// Human-readable description (shown to the *model* — keep it short
    /// and informative; the LLM uses it to decide whether to delegate).
    fn description(&self) -> &str {
        ""
    }

    /// Direct sub-agents (children in the agent tree).
    fn sub_agents(&self) -> &[Arc<dyn BaseAgent>] {
        &[]
    }

    /// Resolve a descendant by name (depth-first).
    fn find_agent(&self, name: &str) -> Option<Arc<dyn BaseAgent>> {
        for sub in self.sub_agents() {
            if sub.name() == name {
                return Some(sub.clone());
            }
            if let Some(found) = sub.find_agent(name) {
                return Some(found);
            }
        }
        None
    }

    /// Run the agent within the given invocation context. Emits a stream of
    /// events. The stream terminates when the agent has produced its final
    /// response (or errors out).
    async fn run(self: Arc<Self>, ctx: Arc<InvocationContext>) -> Result<EventStream<'static>>;
}
