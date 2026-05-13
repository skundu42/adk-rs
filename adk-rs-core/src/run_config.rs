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
}
