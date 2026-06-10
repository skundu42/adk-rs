//! Text-embedding abstraction for semantic memory.
//!
//! An [`Embedder`] turns text into dense vectors so memory backends can rank
//! entries by meaning instead of substring overlap. Provider-backed
//! implementations live behind the provider features
//! ([`GeminiEmbedder`](crate::providers::gemini::GeminiEmbedder),
//! [`OpenAiEmbedder`](crate::providers::openai::OpenAiEmbedder)); the
//! [`VectorMemoryService`](crate::services::mem::VectorMemoryService)
//! consumes any of them.

use async_trait::async_trait;

use crate::error::Result;

/// Computes dense vector embeddings for batches of text.
#[async_trait]
pub trait Embedder: Send + Sync + std::fmt::Debug + 'static {
    /// Embed each input text. The output has one vector per input, in input
    /// order; every vector has the same dimensionality.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// Cosine similarity in `[-1, 1]`. Returns `0.0` when either vector is empty,
/// zero, or the lengths differ.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basics() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(close(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), 1.0));
        assert!(close(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0));
        assert!(close(cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]), -1.0));
        // Degenerate inputs.
        assert!(close(cosine_similarity(&[], &[]), 0.0));
        assert!(close(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0));
        assert!(close(cosine_similarity(&[0.0, 0.0], &[1.0, 2.0]), 0.0));
    }
}
