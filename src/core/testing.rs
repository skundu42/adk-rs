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

/// No-op [`SessionService`](crate::core::SessionService) for tests that
/// need an [`InvocationContext`](crate::core::InvocationContext) but never
/// touch persistence.
#[derive(Debug, Default)]
pub struct NoopSessionService;

#[async_trait]
impl crate::core::SessionService for NoopSessionService {
    async fn create_session(
        &self,
        app: &str,
        user: &str,
        _state: Option<crate::core::State>,
        id: Option<&str>,
    ) -> Result<crate::core::Session> {
        Ok(crate::core::Session::new(app, user, id.unwrap_or("s")))
    }
    async fn get_session(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: crate::core::GetSessionConfig,
    ) -> Result<Option<crate::core::Session>> {
        Ok(None)
    }
    async fn list_sessions(&self, _: &str, _: &str) -> Result<crate::core::ListSessionsResponse> {
        Ok(crate::core::ListSessionsResponse::default())
    }
    async fn delete_session(&self, _: &str, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
}

/// Minimal [`InvocationContext`](crate::core::InvocationContext) backed by
/// [`NoopSessionService`], for driving tools/agents in tests. Mutate the
/// returned value to attach services or user content.
pub fn test_invocation_context() -> crate::core::InvocationContext {
    crate::core::InvocationContext {
        app_name: "app".into(),
        user_id: "u".into(),
        invocation_id: "inv-1".into(),
        session: Arc::new(Mutex::new(crate::core::Session::new("app", "u", "s"))),
        session_service: Arc::new(NoopSessionService),
        artifact_service: None,
        memory_service: None,
        credential_service: None,
        run_config: Default::default(),
        origin: Default::default(),
        user_content: None,
        llm_call_count: Arc::new(Mutex::new(0)),
        cancellation: Default::default(),
        attributes: Arc::new(Mutex::new(Default::default())),
        root_agent: None,
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
