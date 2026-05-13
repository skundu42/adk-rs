//! In-memory [`MemoryService`](crate::core::MemoryService).
//!
//! The implementation does a case-insensitive substring search over the text
//! parts of each event's content. Good enough for tests and quickstart;
//! for production-grade semantic search, swap in a real vector store.

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;

use crate::core::{MemoryEntry, MemoryService, SearchMemoryResponse, Session};
use crate::error::Result;
use crate::genai_types::Content;

#[derive(Debug, Default)]
struct Bucket {
    entries: Vec<MemoryEntry>,
}

/// Volatile memory store.
#[derive(Debug, Default)]
pub struct InMemoryMemoryService {
    by_user: DashMap<(String, String), Mutex<Bucket>>,
}

impl InMemoryMemoryService {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MemoryService for InMemoryMemoryService {
    async fn add_session_to_memory(&self, session: &Session) -> Result<()> {
        let key = (session.app_name.clone(), session.user_id.clone());
        let bucket = self.by_user.entry(key).or_default();
        let mut g = bucket.lock();
        for ev in &session.events {
            if let Some(c) = &ev.response.content {
                let text = c.text_concat();
                if text.is_empty() {
                    continue;
                }
                g.entries.push(MemoryEntry {
                    content: Content::model_text(text),
                    author: Some(ev.author.clone()),
                    timestamp: Some(ev.timestamp),
                });
            }
        }
        Ok(())
    }

    async fn search_memory(
        &self,
        app_name: &str,
        user_id: &str,
        query: &str,
    ) -> Result<SearchMemoryResponse> {
        let key = (app_name.to_string(), user_id.to_string());
        let q = query.to_lowercase();
        let Some(b) = self.by_user.get(&key) else {
            return Ok(SearchMemoryResponse::default());
        };
        let g = b.lock();
        let memories = g
            .entries
            .iter()
            .filter(|e| e.content.text_concat().to_lowercase().contains(&q))
            .cloned()
            .collect();
        Ok(SearchMemoryResponse { memories })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Event;

    #[tokio::test]
    async fn add_then_search_finds_hits() {
        let svc = InMemoryMemoryService::new();
        let mut s = Session::new("app", "u", "s");
        s.events.push(Event::model_text("a", "I like apples"));
        s.events.push(Event::model_text("a", "and oranges"));
        svc.add_session_to_memory(&s).await.unwrap();
        let r = svc.search_memory("app", "u", "Apple").await.unwrap();
        assert_eq!(r.memories.len(), 1);
    }

    #[tokio::test]
    async fn search_returns_empty_when_unseen() {
        let svc = InMemoryMemoryService::new();
        let r = svc.search_memory("nope", "nope", "x").await.unwrap();
        assert!(r.memories.is_empty());
    }
}
