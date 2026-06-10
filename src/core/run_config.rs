//! Per-invocation runtime configuration.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Streaming behaviour requested by the caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamingMode {
    /// One final response only (no partials).
    #[default]
    None,
    /// Server-sent events: token-by-token deltas terminated by a final event.
    Sse,
}

/// Whether paused invocations can be resumed in place (mirrors Python
/// `ResumabilityConfig`). With resumability on, workflow agents record
/// checkpoints (`actions.agent_state`) as sub-agents complete, and
/// `Runner::resume` continues a paused invocation from the last checkpoint
/// instead of restarting the pipeline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumabilityConfig {
    /// Enable pause/resume of invocations.
    #[serde(default)]
    pub is_resumable: bool,
}

/// Per-invocation runtime configuration overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunConfig {
    /// Streaming mode.
    #[serde(default)]
    pub streaming_mode: StreamingMode,
    /// Stop after this many LLM turns. None = unlimited (use with care).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_llm_calls: Option<u32>,
    /// Optional per-invocation custom metadata to merge into emitted events.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_metadata: IndexMap<String, Value>,
    /// Override agent's `model` for this invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// Explicit context-caching config. Usually inherited from the runner
    /// (see `Runner::builder().context_cache_config(..)`); set here to
    /// override per invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_cache_config: Option<crate::core::cache::ContextCacheConfig>,
    /// Pause/resume behaviour. Usually inherited from the runner (see
    /// `Runner::builder().resumable(..)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumability: Option<ResumabilityConfig>,
}
