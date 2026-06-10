//! Context-caching configuration and metadata (mirrors Python ADK's
//! `ContextCacheConfig` / `CacheMetadata`).
//!
//! When a [`ContextCacheConfig`] is attached to a runner (or set per
//! invocation via [`crate::core::RunConfig`]), cache-capable providers (today:
//! Gemini) create an explicit server-side cache for the stable request prefix
//! — system instruction + tool declarations — and reference it on subsequent
//! calls instead of resending the prefix. This cuts cost and latency for
//! agents with large instructions/tool sets.
//!
//! Pair this with [`LlmAgent::static_instruction`](crate::agents::LlmAgent)
//! so the cached prefix stays byte-identical across turns.

use serde::{Deserialize, Serialize};

/// Configuration for explicit provider-side context caching.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextCacheConfig {
    /// Maximum number of LLM calls served by one cache entry before it is
    /// refreshed (guards against unbounded staleness of TTL extensions).
    #[serde(default = "default_cache_intervals")]
    pub cache_intervals: u32,
    /// Cache entry time-to-live, in seconds.
    #[serde(default = "default_ttl_seconds")]
    pub ttl_seconds: u64,
    /// Minimum *estimated* token size of the cacheable prefix. Prefixes that
    /// estimate below this are sent inline (caching tiny prefixes costs more
    /// than it saves; Gemini also enforces a server-side minimum).
    #[serde(default)]
    pub min_tokens: u64,
}

fn default_cache_intervals() -> u32 {
    10
}
fn default_ttl_seconds() -> u64 {
    1800
}

impl Default for ContextCacheConfig {
    fn default() -> Self {
        Self {
            cache_intervals: default_cache_intervals(),
            ttl_seconds: default_ttl_seconds(),
            min_tokens: 0,
        }
    }
}

/// Metadata about cache usage attached to an [`crate::core::LlmResponse`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheMetadata {
    /// Provider-side cache resource name (e.g. `cachedContents/abc123`).
    pub cache_name: String,
    /// Whether this response was served against an existing cache entry
    /// (`true`) or the entry was created for this call (`false`).
    pub cache_hit: bool,
}
