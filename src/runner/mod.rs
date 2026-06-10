//! Runner orchestrator for adk-rs.

mod compaction;
mod plugin;
mod runner;

pub use compaction::{EventSummarizer, EventsCompactionConfig, LlmEventSummarizer};
pub use plugin::{BasePlugin, LoggingPlugin, PluginManager};
pub use runner::{Runner, RunnerBuilder, RunningInvocation};
