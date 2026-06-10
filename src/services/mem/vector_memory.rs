//! Embedding-backed in-memory [`MemoryService`]: cosine-ranked semantic
//! search instead of substring match.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;

use crate::core::embedder::{Embedder, cosine_similarity};
use crate::core::memory::{MemoryEntry, SearchMemoryResponse};
use crate::core::services::MemoryService;
use crate::core::session::Session;
use crate::error::Result;
use crate::genai_types::Content;

#[derive(Debug, Default)]
struct Bucket {
    entries: Vec<(MemoryEntry, Vec<f32>)>,
}

/// In-memory [`MemoryService`] that embeds entries with an [`Embedder`] at
/// ingest time and ranks search results by cosine similarity.
///
/// Storage is process-local, like
/// [`InMemoryMemoryService`](super::InMemoryMemoryService) — the upgrade is
/// the *retrieval* quality, not durability. Pair it with any embedder:
/// `GeminiEmbedder`, `OpenAiEmbedder`, or your own.
///
/// ```no_run
/// # async fn demo() -> adk_rs::Result<()> {
/// # #[cfg(feature = "gemini")] {
/// use adk_rs::providers::gemini::GeminiEmbedder;
/// use adk_rs::services::mem::VectorMemoryService;
/// use std::sync::Arc;
///
/// let svc = VectorMemoryService::new(Arc::new(GeminiEmbedder::from_env(
///     "gemini-embedding-001",
/// )?))
/// .with_top_k(5)
/// .with_min_score(0.3);
/// # }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct VectorMemoryService {
    embedder: Arc<dyn Embedder>,
    /// `(app_name, user_id)` → embedded entries.
    by_user: DashMap<(String, String), Arc<Mutex<Bucket>>>,
    top_k: usize,
    min_score: f32,
}

impl VectorMemoryService {
    /// Construct with the given embedder, returning at most 5 results per
    /// search and no similarity floor.
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            embedder,
            by_user: DashMap::new(),
            top_k: 5,
            min_score: 0.0,
        }
    }

    /// Maximum number of results per search (default 5).
    pub fn with_top_k(mut self, k: usize) -> Self {
        self.top_k = k.max(1);
        self
    }

    /// Minimum cosine similarity for a result to be returned (default 0.0).
    pub fn with_min_score(mut self, score: f32) -> Self {
        self.min_score = score;
        self
    }
}

#[async_trait]
impl MemoryService for VectorMemoryService {
    async fn add_session_to_memory(&self, session: &Session) -> Result<()> {
        let mut entries: Vec<MemoryEntry> = Vec::new();
        let mut texts: Vec<String> = Vec::new();
        for ev in &session.events {
            if let Some(c) = &ev.response.content {
                let text = c.text_concat();
                if text.is_empty() {
                    continue;
                }
                texts.push(text.clone());
                entries.push(MemoryEntry {
                    content: Content::model_text(text),
                    author: Some(ev.author.clone()),
                    timestamp: Some(ev.timestamp),
                });
            }
        }
        if entries.is_empty() {
            return Ok(());
        }
        let vectors = self.embedder.embed(&texts).await?;
        let key = (session.app_name.clone(), session.user_id.clone());
        let bucket = self.by_user.entry(key).or_default().clone();
        let mut g = bucket.lock();
        g.entries.extend(entries.into_iter().zip(vectors));
        Ok(())
    }

    async fn search_memory(
        &self,
        app_name: &str,
        user_id: &str,
        query: &str,
    ) -> Result<SearchMemoryResponse> {
        let key = (app_name.to_string(), user_id.to_string());
        let Some(bucket) = self.by_user.get(&key).map(|b| b.clone()) else {
            return Ok(SearchMemoryResponse::default());
        };
        let qv = self
            .embedder
            .embed(std::slice::from_ref(&query.to_string()))
            .await?
            .into_iter()
            .next()
            .unwrap_or_default();

        let mut scored: Vec<(f32, MemoryEntry)> = {
            let g = bucket.lock();
            g.entries
                .iter()
                .map(|(e, v)| (cosine_similarity(&qv, v), e.clone()))
                .filter(|(s, _)| *s >= self.min_score)
                .collect()
        };
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(self.top_k);
        Ok(SearchMemoryResponse {
            memories: scored.into_iter().map(|(_, e)| e).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Event;
    use crate::core::testing::MockEmbedder;

    fn service() -> VectorMemoryService {
        VectorMemoryService::new(Arc::new(MockEmbedder::default()))
    }

    #[tokio::test]
    async fn ranks_semantically_similar_entries_first() {
        let svc = service().with_top_k(1);
        let mut s = Session::new("app", "u", "s");
        s.events
            .push(Event::model_text("a", "the cat sat on the mat"));
        s.events
            .push(Event::model_text("a", "stock prices fell sharply"));
        svc.add_session_to_memory(&s).await.unwrap();

        let r = svc.search_memory("app", "u", "cat on a mat").await.unwrap();
        assert_eq!(r.memories.len(), 1);
        assert!(
            r.memories[0]
                .content
                .text_concat()
                .contains("cat sat on the mat")
        );
    }

    #[tokio::test]
    async fn min_score_filters_unrelated_entries() {
        let svc = service().with_min_score(0.99);
        let mut s = Session::new("app", "u", "s");
        s.events
            .push(Event::model_text("a", "completely unrelated"));
        svc.add_session_to_memory(&s).await.unwrap();
        let r = svc.search_memory("app", "u", "cat").await.unwrap();
        assert!(r.memories.is_empty());
    }

    #[tokio::test]
    async fn scoped_per_app_and_user() {
        let svc = service();
        let mut s = Session::new("app", "alice", "s");
        s.events.push(Event::model_text("a", "alice likes apples"));
        svc.add_session_to_memory(&s).await.unwrap();
        let r = svc.search_memory("app", "bob", "apples").await.unwrap();
        assert!(r.memories.is_empty());
    }

    #[tokio::test]
    async fn empty_store_returns_empty() {
        let svc = service();
        let r = svc.search_memory("app", "u", "anything").await.unwrap();
        assert!(r.memories.is_empty());
    }
}
