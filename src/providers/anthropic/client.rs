//! Anthropic HTTP client.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tracing::instrument;

use crate::core::stream::LlmResponseStream;
use crate::core::{LlmRequest, LlmResponse, Model};
use crate::error::{Error, ProviderError, Result};

use crate::providers::anthropic::convert::{parse_response, to_wire};

/// Configuration.
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    /// Base URL (default: `https://api.anthropic.com`).
    pub base_url: String,
    /// `anthropic-version` header.
    pub anthropic_version: String,
    /// API key.
    pub api_key: String,
    /// HTTP request timeout.
    pub timeout: Duration,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".into(),
            anthropic_version: "2023-06-01".into(),
            api_key: String::new(),
            timeout: Duration::from_secs(60),
        }
    }
}

/// Anthropic provider.
#[derive(Debug, Clone)]
pub struct Anthropic {
    model_name: String,
    cfg: AnthropicConfig,
    http: Client,
}

impl Anthropic {
    /// Construct.
    pub fn new(model_name: impl Into<String>, cfg: AnthropicConfig) -> Result<Self> {
        let http = Client::builder()
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

    /// Construct from `$ANTHROPIC_API_KEY`.
    pub fn from_env(model_name: impl Into<String>) -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::config("ANTHROPIC_API_KEY env var not set"))?;
        Self::new(
            model_name,
            AnthropicConfig {
                api_key,
                ..AnthropicConfig::default()
            },
        )
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.cfg.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Model for Anthropic {
    fn name(&self) -> &str {
        &self.model_name
    }

    fn supported_models(&self) -> &'static [&'static str] {
        &["claude-*"]
    }

    #[instrument(skip(self, req), fields(model = %self.model_name))]
    async fn generate_content(&self, req: LlmRequest) -> Result<LlmResponse> {
        if self.cfg.api_key.is_empty() {
            return Err(Error::Provider(ProviderError::Auth(
                "ANTHROPIC_API_KEY is empty".into(),
            )));
        }
        let body = serde_json::to_vec(&to_wire(&req, &self.model_name))?;
        let resp = self
            .http
            .post(self.endpoint())
            .header("x-api-key", &self.cfg.api_key)
            .header("anthropic-version", &self.cfg.anthropic_version)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
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
        parse_response(&bytes)
    }

    async fn stream_generate_content(&self, req: LlmRequest) -> Result<LlmResponseStream> {
        // For v0.1, fall back to single-shot then yield once. Real SSE event
        // accumulation lands in a follow-up.
        let r = self.generate_content(req).await?;
        Ok(crate::providers::anthropic::stream_one(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type":"text","text":"hi"}],
                "stop_reason": "end_turn",
                "model": "claude-test",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })))
            .mount(&server)
            .await;

        let a = Anthropic::new(
            "claude-3-5-sonnet",
            AnthropicConfig {
                base_url: server.uri(),
                api_key: "k".into(),
                ..AnthropicConfig::default()
            },
        )
        .unwrap();
        let req = LlmRequest {
            contents: vec![crate::genai_types::Content::user_text("hi")],
            ..Default::default()
        };
        let r = a.generate_content(req).await.unwrap();
        assert_eq!(r.content.unwrap().text_concat(), "hi");
    }
}
