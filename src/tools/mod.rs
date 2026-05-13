//! Tools for adk-rs. Provides the public [`Tool`] alias (re-exported from
//! `crate::core::DynTool`), a [`FunctionTool`] wrapper, and built-in tools.

mod builtin;
mod function_tool;
mod toolset;

pub use builtin::{exit_loop, transfer_to_agent_tool};
pub use function_tool::FunctionTool;
pub use toolset::{StaticToolset, Toolset};

/// The user-facing `Tool` trait. Same as [`crate::core::DynTool`].
pub use crate::core::DynTool as Tool;

/// `#[tool]` attribute macro (available with the `macros` feature).
#[cfg(feature = "macros")]
pub use adk_rs_macros::tool;
