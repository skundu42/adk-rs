//! OpenAI-compatible text-embedding client (`/embeddings`).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::core::embedder::Embedder;
use crate::error::{Error, ProviderError, Result};
use crate::providers::common::send_with_retry;
use crate::providers::openai::OpenAiConfig;

/// [`Embedder`] backed by an OpenAI-compatible `/embeddings` endpoint —
/// OpenAI itself (`text-embedding-3-small`/`-large`), Azure OpenAI, Ollama,
/// or any other server honouring the same wire shape, selected via
/// `OpenAiConfig.base_url`.
///
/// ```no_run
/// # fn main() -> adk_rs::Result<()> {
/// use adk_rs::providers::openai::OpenAiEmbedder;
/// let embedder = OpenAiEmbedder::from_env("text-embedding-3-small")?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct OpenAiEmbedder {
    model_name: String,
    cfg: OpenAiConfig,
    http: reqwest::Client,
}

impl OpenAiEmbedder {
    /// Construct from config and an embedding model name.
    pub fn new(model_name: impl Into<String>, cfg: OpenAiConfig) -> Result<Self> {
        crate::transport_security::require_secure_url(&cfg.base_url, "OpenAiConfig.base_url")?;
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .user_agent(concat!("adk-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        Ok(Self {
            model_name: model_name.into(),
            cfg,
            http,
        })
    }

    /// Construct from `$OPENAI_API_KEY` and optional `$OPENAI_BASE_URL`.
    pub fn from_env(model_name: impl Into<String>) -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| Error::config("OPENAI_API_KEY env var not set"))?;
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        Self::new(
            model_name,
            OpenAiConfig {
                api_key,
                base_url,
                ..OpenAiConfig::default()
            },
        )
    }

    fn endpoint(&self) -> String {
        let mut url = format!("{}/embeddings", self.cfg.base_url.trim_end_matches('/'));
        if let Some(v) = &self.cfg.api_version {
            url.push_str(if url.contains('?') { "&" } else { "?" });
            url.push_str("api-version=");
            url.push_str(v);
        }
        url
    }
}

#[derive(Deserialize)]
struct WireEmbeddingsResponse {
    data: Vec<WireEmbeddingItem>,
}

#[derive(Deserialize)]
struct WireEmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        if self.cfg.api_key.is_empty() {
            return Err(Error::Provider(ProviderError::Auth(
                "OPENAI_API_KEY is empty".into(),
            )));
        }
        let body = serde_json::to_vec(&json!({
            "model": self.model_name,
            "input": texts,
        }))?;
        let resp = send_with_retry(&self.cfg.retry, || {
            let mut rb = self
                .http
                .post(self.endpoint())
                .header("authorization", format!("Bearer {}", self.cfg.api_key))
                .header("content-type", "application/json");
            if let Some(org) = &self.cfg.organization {
                rb = rb.header("openai-organization", org);
            }
            rb.body(body.clone()).send()
        })
        .await?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(Error::Provider(ProviderError::Http {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes).to_string(),
            }));
        }
        let r: WireEmbeddingsResponse = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Decode(format!("openai embeddings: {e}")))?;
        if r.data.len() != texts.len() {
            return Err(Error::Provider(ProviderError::Decode(format!(
                "expected {} embeddings, got {}",
                texts.len(),
                r.data.len()
            ))));
        }
        // The API documents `index`; sort defensively rather than assume
        // response order.
        let mut data = r.data;
        data.sort_by_key(|d| d.index);
        Ok(data.into_iter().map(|d| d.embedding).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn embeddings_happy_path_sorted_by_index() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .and(header("authorization", "Bearer k"))
            .and(body_partial_json(serde_json::json!({
                "model": "text-embedding-3-small",
                "input": ["a", "b"]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                // Out of order on purpose — must be re-sorted by index.
                "data": [
                    {"index": 1, "embedding": [0.3, 0.4]},
                    {"index": 0, "embedding": [0.1, 0.2]}
                ],
                "model": "text-embedding-3-small",
                "usage": {"prompt_tokens": 2, "total_tokens": 2}
            })))
            .mount(&server)
            .await;
        let e = OpenAiEmbedder::new(
            "text-embedding-3-small",
            OpenAiConfig {
                base_url: server.uri(),
                api_key: "k".into(),
                ..OpenAiConfig::default()
            },
        )
        .unwrap();
        let v = e.embed(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(v, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }
}
