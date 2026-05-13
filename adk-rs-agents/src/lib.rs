//! Agent abstractions for adk-rs.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod base;
mod llm_agent;
mod loop_agent;
mod parallel_agent;
mod sequential_agent;

pub use base::BaseAgent;
pub use llm_agent::{DEFAULT_MODEL, InstructionProvider, LlmAgent, LlmAgentBuilder};
pub use loop_agent::LoopAgent;
pub use parallel_agent::ParallelAgent;
pub use sequential_agent::SequentialAgent;

/// Re-export of `adk_rs_core::DynTool` so users see one `Tool` name.
pub use adk_rs_core::DynTool as Tool;
