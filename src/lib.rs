//! `adk-rs` — Rust port of the [Google Agent Development Kit](https://github.com/google/adk-python).
//!
//! This crate is a single, feature-gated front door over what was originally a
//! workspace of 17 sub-crates. See the [README] for a guided tour.
//!
//! [README]: https://github.com/skundu42/adk-rs

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]
#![allow(clippy::module_name_repetitions)]

pub mod error;
pub mod genai_types;
pub mod core;
pub mod services;
pub mod providers;
pub mod tools;
pub mod agents;
pub mod runner;

#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "telemetry")]
pub mod telemetry;
#[cfg(feature = "eval")]
pub mod eval;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "cli")]
pub mod cli;

// Convenience re-exports for the most common types.
pub use error::{Error, Result};
pub use agents::{BaseAgent, LlmAgent, LoopAgent, ParallelAgent, SequentialAgent};
pub use runner::Runner;
pub use tools::Tool;

/// Items the `#[tool]` proc-macro emits absolute paths into. Hidden from docs;
/// not a stable public API.
#[doc(hidden)]
#[cfg(feature = "macros")]
pub mod __private {
    pub use crate::core::{DynTool, ToolContext};
    pub use crate::error::{Error, Result, ToolError};
    pub use crate::genai_types::{FunctionDeclaration, Schema};
}

#[cfg(feature = "macros")]
pub use adk_rs_macros::tool;
