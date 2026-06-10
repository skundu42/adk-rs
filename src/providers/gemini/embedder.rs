//! Gemini text-embedding client (`batchEmbedContents`).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::core::embedder::Embedder;
use crate::error::{Error, ProviderError, Result};
use crate::providers::common::send_with_retry;
use crate::providers::gemini::GeminiConfig;

/// [`Embedder`] backed by the Gemini embeddings API
/// (`models/{model}:batchEmbedContents`).
///
/// ```no_run
/// # fn main() -> adk_rs::Result<()> {
/// use adk_rs::providers::gemini::GeminiEmbedder;
/// let embedder = GeminiEmbedder::from_env("gemini-embedding-001")?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct GeminiEmbedder {
    model_name: String,
    cfg: GeminiConfig,
    http: reqwest::Client,
}

impl GeminiEmbedder {
    /// Construct from config and an embedding model name (e.g.
    /// `gemini-embedding-001`).
    pub fn new(model_name: impl Into<String>, cfg: GeminiConfig) -> Result<Self> {
        crate::transport_security::require_secure_url(&cfg.base_url, "GeminiConfig.base_url")?;
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

    /// Construct from `$GOOGLE_API_KEY`.
    pub fn from_env(model_name: impl Into<String>) -> Result<Self> {
        let api_key = std::env::var("GOOGLE_API_KEY")
            .map_err(|_| Error::config("GOOGLE_API_KEY env var not set"))?;
        Self::new(
            model_name,
            GeminiConfig {
                api_key,
                ..GeminiConfig::default()
            },
        )
    }
}

#[derive(Deserialize)]
struct WireBatchEmbedResponse {
    embeddings: Vec<WireEmbedding>,
}

#[derive(Deserialize)]
struct WireEmbedding {
    values: Vec<f32>,
}

#[async_trait]
impl Embedder for GeminiEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        if self.cfg.api_key.is_empty() {
            return Err(Error::Provider(ProviderError::Auth(
                "Gemini api_key is empty; set $GOOGLE_API_KEY".into(),
            )));
        }
        let url = format!(
            "{}/{}/models/{}:batchEmbedContents",
            self.cfg.base_url.trim_end_matches('/'),
            self.cfg.api_version,
            self.model_name,
        );
        let model = format!("models/{}", self.model_name);
        let requests: Vec<_> = texts
            .iter()
            .map(|t| json!({"model": &model, "content": {"parts": [{"text": t}]}}))
            .collect();
        let body = serde_json::to_vec(&json!({ "requests": requests }))?;

        let resp = send_with_retry(&self.cfg.retry, || {
            self.http
                .post(&url)
                .header("x-goog-api-key", &self.cfg.api_key)
                .header("content-type", "application/json")
                .body(body.clone())
                .send()
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
        let r: WireBatchEmbedResponse = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Decode(format!("gemini embeddings: {e}")))?;
        if r.embeddings.len() != texts.len() {
            return Err(Error::Provider(ProviderError::Decode(format!(
                "expected {} embeddings, got {}",
                texts.len(),
                r.embeddings.len()
            ))));
        }
        Ok(r.embeddings.into_iter().map(|e| e.values).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn batch_embed_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v1beta/models/gemini-embedding-001:batchEmbedContents",
            ))
            .and(body_partial_json(serde_json::json!({
                "requests": [
                    {"model": "models/gemini-embedding-001",
                     "content": {"parts": [{"text": "hello"}]}},
                    {"model": "models/gemini-embedding-001",
                     "content": {"parts": [{"text": "world"}]}}
                ]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [{"values": [0.1, 0.2]}, {"values": [0.3, 0.4]}]
            })))
            .mount(&server)
            .await;
        let e = GeminiEmbedder::new(
            "gemini-embedding-001",
            GeminiConfig {
                base_url: server.uri(),
                api_key: "k".into(),
                ..GeminiConfig::default()
            },
        )
        .unwrap();
        let v = e.embed(&["hello".into(), "world".into()]).await.unwrap();
        assert_eq!(v, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }

    #[tokio::test]
    async fn empty_input_short_circuits() {
        let e = GeminiEmbedder::new(
            "gemini-embedding-001",
            GeminiConfig {
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                ..GeminiConfig::default()
            },
        )
        .unwrap();
        assert!(e.embed(&[]).await.unwrap().is_empty());
    }
}
