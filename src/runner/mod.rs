//! Runner orchestrator for adk-rs.

mod plugin;
mod runner;

pub use plugin::{BasePlugin, LoggingPlugin, PluginManager};
pub use runner::{Runner, RunnerBuilder, RunningInvocation};
