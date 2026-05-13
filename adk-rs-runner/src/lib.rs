//! Runner orchestrator for adk-rs.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod plugin;
mod runner;

pub use plugin::{BasePlugin, LoggingPlugin, PluginManager};
pub use runner::{Runner, RunnerBuilder};
