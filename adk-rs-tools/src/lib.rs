//! Tools for adk-rs. Provides the public [`Tool`] alias (re-exported from
//! `adk_rs_core::DynTool`), a [`FunctionTool`] wrapper, and built-in tools.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod builtin;
mod function_tool;
mod toolset;

pub use builtin::{exit_loop, transfer_to_agent_tool};
pub use function_tool::FunctionTool;
pub use toolset::{StaticToolset, Toolset};

/// The user-facing `Tool` trait. Same as [`adk_rs_core::DynTool`].
pub use adk_rs_core::DynTool as Tool;

/// `#[adk::tool]` attribute macro.
pub use adk_rs_tools_macros::tool;
