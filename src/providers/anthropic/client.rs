//! Anthropic HTTP client.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tracing::instrument;

use crate::core::retry::RetryConfig;
use crate::core::stream::LlmResponseStream;
use crate::core::{LlmRequest, LlmResponse, Model};
use crate::error::{Error, ProviderError, Result};
use crate::providers::common::send_with_retry;

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
    /// Retry policy for transient failures (429 / 529 / 5xx / connect errors).
    pub retry: RetryConfig,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".into(),
            anthropic_version: "2023-06-01".into(),
            api_key: String::new(),
            timeout: Duration::from_secs(60),
            retry: RetryConfig::default(),
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
        crate::transport_security::require_secure_url(&cfg.base_url, "AnthropicConfig.base_url")?;
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
        let resp = send_with_retry(&self.cfg.retry, || {
            self.http
                .post(self.endpoint())
                .header("x-api-key", &self.cfg.api_key)
                .header("anthropic-version", &self.cfg.anthropic_version)
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
        parse_response(&bytes)
    }

    async fn stream_generate_content(&self, req: LlmRequest) -> Result<LlmResponseStream> {
        if self.cfg.api_key.is_empty() {
            return Err(Error::Provider(ProviderError::Auth(
                "ANTHROPIC_API_KEY is empty".into(),
            )));
        }
        let mut wire = to_wire(&req, &self.model_name);
        wire.stream = true;
        let body = serde_json::to_vec(&wire)?;
        let resp = send_with_retry(&self.cfg.retry, || {
            self.http
                .post(self.endpoint())
                .header("x-api-key", &self.cfg.api_key)
                .header("anthropic-version", &self.cfg.anthropic_version)
                .header("content-type", "application/json")
                .body(body.clone())
                .send()
        })
        .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
            return Err(Error::Provider(ProviderError::Http { status, body }));
        }
        Ok(crate::providers::anthropic::stream::from_sse(resp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn rejects_plaintext_http_base_url() {
        let err = Anthropic::new(
            "claude-3-5-sonnet",
            AnthropicConfig {
                base_url: "http://api.example.com".into(),
                api_key: "k".into(),
                ..AnthropicConfig::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("https"));
    }

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

    #[tokio::test]
    async fn streaming_decodes_text_tool_calls_and_usage() {
        use futures::TryStreamExt;
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-test\",\"usage\":{\"input_tokens\":7}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu-1\",\"name\":\"f\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"x\\\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\":1}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse),
            )
            .mount(&server)
            .await;
        let a = Anthropic::new(
            "claude-sonnet-4-6",
            AnthropicConfig {
                base_url: server.uri(),
                api_key: "k".into(),
                ..AnthropicConfig::default()
            },
        )
        .unwrap();
        let stream = a
            .stream_generate_content(LlmRequest {
                contents: vec![crate::genai_types::Content::user_text("hi")],
                ..Default::default()
            })
            .await
            .unwrap();
        let chunks: Vec<_> = stream.try_collect().await.unwrap();

        // Two text deltas, one complete tool call, one final chunk.
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].content.as_ref().unwrap().text_concat(), "Hel");
        assert_eq!(chunks[1].content.as_ref().unwrap().text_concat(), "lo");
        let calls = chunks[2].function_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id.as_deref(), Some("tu-1"));
        assert_eq!(calls[0].args["x"], 1);
        let last = &chunks[3];
        assert_eq!(
            last.finish_reason,
            Some(crate::genai_types::FinishReason::Stop)
        );
        let usage = last.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(7));
        assert_eq!(usage.candidates_token_count, Some(3));
    }
}
