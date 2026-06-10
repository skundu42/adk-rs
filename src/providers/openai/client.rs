//! OpenAI-compatible HTTP client.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tracing::instrument;

use crate::core::retry::RetryConfig;
use crate::core::stream::LlmResponseStream;
use crate::core::{LlmRequest, LlmResponse, Model};
use crate::error::{Error, ProviderError, Result};
use crate::providers::common::send_with_retry;

use crate::providers::openai::convert::{parse_response, to_wire};

/// Configuration.
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Base URL (default: `https://api.openai.com/v1`).
    pub base_url: String,
    /// API key (`Authorization: Bearer ...`).
    pub api_key: String,
    /// Optional Azure-style `api-version`.
    pub api_version: Option<String>,
    /// Optional org id (`OpenAI-Organization` header).
    pub organization: Option<String>,
    /// HTTP timeout.
    pub timeout: Duration,
    /// Retry policy for transient failures (429 / 5xx / connect errors).
    pub retry: RetryConfig,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            api_version: None,
            organization: None,
            timeout: Duration::from_secs(60),
            retry: RetryConfig::default(),
        }
    }
}

/// OpenAI-compatible provider.
#[derive(Debug, Clone)]
pub struct OpenAi {
    model_name: String,
    cfg: OpenAiConfig,
    http: Client,
}

impl OpenAi {
    /// Construct.
    pub fn new(model_name: impl Into<String>, cfg: OpenAiConfig) -> Result<Self> {
        crate::transport_security::require_secure_url(&cfg.base_url, "OpenAiConfig.base_url")?;
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
        let mut url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        if let Some(v) = &self.cfg.api_version {
            url.push_str(if url.contains('?') { "&" } else { "?" });
            url.push_str("api-version=");
            url.push_str(v);
        }
        url
    }
}

#[async_trait]
impl Model for OpenAi {
    fn name(&self) -> &str {
        &self.model_name
    }

    fn supported_models(&self) -> &'static [&'static str] {
        // Match anything; we let the user pick what their base_url accepts.
        &[
            "openai/*", "gpt-*", "o1-*", "o3-*", "azure/*", "ollama/*", "groq/*",
        ]
    }

    #[instrument(skip(self, req), fields(model = %self.model_name))]
    async fn generate_content(&self, req: LlmRequest) -> Result<LlmResponse> {
        if self.cfg.api_key.is_empty() {
            return Err(Error::Provider(ProviderError::Auth(
                "OPENAI_API_KEY is empty".into(),
            )));
        }
        let body = serde_json::to_vec(&to_wire(&req, &self.model_name))?;
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
        parse_response(&bytes)
    }

    async fn stream_generate_content(&self, req: LlmRequest) -> Result<LlmResponseStream> {
        if self.cfg.api_key.is_empty() {
            return Err(Error::Provider(ProviderError::Auth(
                "OPENAI_API_KEY is empty".into(),
            )));
        }
        let mut wire = to_wire(&req, &self.model_name);
        wire.stream = true;
        wire.stream_options = Some(serde_json::json!({ "include_usage": true }));
        let body = serde_json::to_vec(&wire)?;
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
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
            return Err(Error::Provider(ProviderError::Http { status, body }));
        }
        Ok(crate::providers::openai::stream::from_sse(resp))
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
        let err = OpenAi::new(
            "gpt-4o-mini",
            OpenAiConfig {
                base_url: "http://api.example.com/v1".into(),
                api_key: "k".into(),
                ..OpenAiConfig::default()
            },
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("https") || msg.contains("loopback"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "gpt-4o-mini",
                "choices": [{"message": {"content": "yo"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            })))
            .mount(&server)
            .await;
        let o = OpenAi::new(
            "gpt-4o-mini",
            OpenAiConfig {
                base_url: server.uri(),
                api_key: "k".into(),
                ..OpenAiConfig::default()
            },
        )
        .unwrap();
        let req = LlmRequest {
            contents: vec![crate::genai_types::Content::user_text("hi")],
            ..Default::default()
        };
        let r = o.generate_content(req).await.unwrap();
        assert_eq!(r.content.unwrap().text_concat(), "yo");
    }

    #[tokio::test]
    async fn streaming_decodes_deltas_tool_calls_and_usage() {
        use futures::TryStreamExt;
        use wiremock::matchers::body_partial_json;
        let sse = concat!(
            "data: {\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"He\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"y\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"type\":\"function\",\"function\":{\"name\":\"f\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"x\\\":1}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n\n",
            "data: [DONE]\n\n",
        );
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(json!({
                "stream": true,
                "stream_options": {"include_usage": true}
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse),
            )
            .mount(&server)
            .await;
        let o = OpenAi::new(
            "gpt-4o-mini",
            OpenAiConfig {
                base_url: server.uri(),
                api_key: "k".into(),
                ..OpenAiConfig::default()
            },
        )
        .unwrap();
        let stream = o
            .stream_generate_content(LlmRequest {
                contents: vec![crate::genai_types::Content::user_text("hi")],
                ..Default::default()
            })
            .await
            .unwrap();
        let chunks: Vec<_> = stream.try_collect().await.unwrap();

        // Two text deltas + one final chunk with the tool call and usage.
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].content.as_ref().unwrap().text_concat(), "He");
        assert_eq!(chunks[1].content.as_ref().unwrap().text_concat(), "y");
        let last = &chunks[2];
        let calls = last.function_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id.as_deref(), Some("call-1"));
        assert_eq!(calls[0].args["x"], 1);
        assert_eq!(
            last.finish_reason,
            Some(crate::genai_types::FinishReason::Stop)
        );
        assert_eq!(last.usage_metadata.unwrap().total_token_count, Some(7));
    }
}
