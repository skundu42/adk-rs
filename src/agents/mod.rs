//! Agent abstractions for adk-rs.

mod base;
pub mod instructions;
mod llm_agent;
mod loop_agent;
mod parallel_agent;
mod sequential_agent;

#[cfg(test)]
pub(crate) mod tests_support;

pub use base::BaseAgent;
pub use instructions::inject_session_state;
pub use llm_agent::{
    DEFAULT_MODEL, IncludeContents, InstructionProvider, LlmAgent, LlmAgentBuilder,
};
pub use loop_agent::LoopAgent;
pub use parallel_agent::ParallelAgent;
pub use sequential_agent::SequentialAgent;

/// Re-export of `crate::core::DynTool` so users see one `Tool` name.
pub use crate::core::DynTool as Tool;
