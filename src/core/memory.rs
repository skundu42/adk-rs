//! Memory entries returned by [`MemoryService::search_memory`].
//!
//! [`MemoryService::search_memory`]: crate::core::services::MemoryService::search_memory

use serde::{Deserialize, Serialize};

use crate::genai_types::Content;

/// A single memory entry. The `content` is the textual snippet, with optional
/// `author` and `timestamp` metadata mirrored from the original event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Memory content (typically a text part).
    pub content: Content,
    /// Original author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Original timestamp (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
}

/// Result envelope for memory search.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchMemoryResponse {
    /// Matching entries.
    #[serde(default)]
    pub memories: Vec<MemoryEntry>,
}
