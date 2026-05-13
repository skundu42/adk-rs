//! Tools for adk-rs. Provides the public [`Tool`] alias (re-exported from
//! `crate::core::DynTool`), a [`FunctionTool`] wrapper, and built-in tools.

mod agent_tool;
mod builtin;
mod function_tool;
mod get_user_choice;
mod load_artifacts;
mod load_memory;
mod long_running;
mod preload_memory;
mod toolset;
mod url_context;

#[cfg(feature = "openapi")]
pub mod openapi;

pub use agent_tool::AgentTool;
pub use builtin::{exit_loop, transfer_to_agent_tool};
pub use function_tool::FunctionTool;
pub use get_user_choice::get_user_choice_tool;
pub use load_artifacts::load_artifacts_tool;
pub use load_memory::load_memory_tool;
pub use long_running::LongRunningFunctionTool;
pub use preload_memory::preload_memory_tool;
pub use toolset::{StaticToolset, Toolset};
pub use url_context::url_context_tool;

/// The user-facing `Tool` trait. Same as [`crate::core::DynTool`].
pub use crate::core::DynTool as Tool;

/// `#[tool]` attribute macro (available with the `macros` feature).
#[cfg(feature = "macros")]
pub use adk_rs_macros::tool;
