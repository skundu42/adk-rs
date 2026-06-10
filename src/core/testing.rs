//! Test helpers gated behind `cfg(test)` (or the `testing` feature for
//! cross-crate use).

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::error::Result;
use crate::genai_types::Content;

use crate::core::llm_request::LlmRequest;
use crate::core::llm_response::LlmResponse;
use crate::core::model::Model;

/// Scripted mock model that returns pre-queued responses in order.
#[derive(Debug)]
pub struct MockModel {
    name: String,
    /// FIFO of responses to return on each `generate_content` call.
    responses: Arc<Mutex<Vec<LlmResponse>>>,
    /// Captured requests, in call order.
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

impl MockModel {
    /// Construct an empty mock (use `push_response` to queue replies).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            responses: Arc::default(),
            requests: Arc::default(),
        }
    }

    /// Queue a response.
    pub fn push_response(&self, r: LlmResponse) {
        self.responses.lock().insert(0, r);
    }

    /// Queue a plain-text response.
    pub fn push_text(&self, text: impl Into<String>) {
        self.push_response(LlmResponse {
            content: Some(Content::model_text(text)),
            ..LlmResponse::default()
        });
    }

    /// Captured requests, in call order.
    pub fn captured_requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().clone()
    }
}

#[async_trait]
impl Model for MockModel {
    fn name(&self) -> &str {
        &self.name
    }

    fn supported_models(&self) -> &'static [&'static str] {
        &["mock-*"]
    }

    async fn generate_content(&self, req: LlmRequest) -> Result<LlmResponse> {
        self.requests.lock().push(req);
        let r =
            self.responses.lock().pop().ok_or_else(|| {
                crate::error::Error::other("MockModel ran out of queued responses")
            })?;
        Ok(r)
    }
}

/// Deterministic, offline [`Embedder`](crate::core::Embedder): a hashed
/// bag-of-words vector. Texts sharing words get high cosine similarity —
/// crude, but stable and dependency-free, which is exactly what tests want.
#[derive(Debug, Default)]
pub struct MockEmbedder {
    /// Vector dimensionality (default 256).
    pub dim: usize,
}

#[async_trait]
impl crate::core::embedder::Embedder for MockEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let dim = if self.dim == 0 { 256 } else { self.dim };
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.0f32; dim];
                for word in t
                    .to_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                {
                    // FNV-1a: deterministic across processes, unlike the
                    // std hasher.
                    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                    for b in word.bytes() {
                        h ^= b as u64;
                        h = h.wrapping_mul(0x0000_0100_0000_01b3);
                    }
                    v[(h % dim as u64) as usize] += 1.0;
                }
                v
            })
            .collect())
    }
}
