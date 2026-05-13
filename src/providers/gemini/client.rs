//! Gemini HTTP client.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, instrument};

use crate::core::stream::LlmResponseStream;
use crate::core::{LlmRequest, LlmResponse, Model};
use crate::error::{Error, ProviderError, Result};

use crate::providers::gemini::convert::{parse_response, to_wire};
use crate::providers::gemini::stream::from_sse;

/// Configuration for [`Gemini`].
#[derive(Debug, Clone)]
pub struct GeminiConfig {
    /// Base API URL (default: `https://generativelanguage.googleapis.com`).
    pub base_url: String,
    /// API version path segment (default: `v1beta`).
    pub api_version: String,
    /// API key (required). If empty, loaded from `$GOOGLE_API_KEY`.
    pub api_key: String,
    /// HTTP request timeout.
    pub timeout: Duration,
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://generativelanguage.googleapis.com".into(),
            api_version: "v1beta".into(),
            api_key: String::new(),
            timeout: Duration::from_secs(60),
        }
    }
}

/// Gemini provider.
#[derive(Debug, Clone)]
pub struct Gemini {
    model_name: String,
    cfg: GeminiConfig,
    http: Client,
}

impl Gemini {
    /// Construct from config and a model name.
    pub fn new(model_name: impl Into<String>, cfg: GeminiConfig) -> Result<Self> {
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

    fn endpoint(&self, action: &str) -> String {
        format!(
            "{}/{}/models/{}:{}",
            self.cfg.base_url.trim_end_matches('/'),
            self.cfg.api_version,
            self.model_name,
            action
        )
    }

    fn auth_header(&self) -> Result<String> {
        if self.cfg.api_key.is_empty() {
            return Err(Error::Provider(ProviderError::Auth(
                "Gemini api_key is empty; set $GOOGLE_API_KEY".into(),
            )));
        }
        Ok(self.cfg.api_key.clone())
    }
}

#[async_trait]
impl Model for Gemini {
    fn name(&self) -> &str {
        &self.model_name
    }

    fn supported_models(&self) -> &'static [&'static str] {
        &["gemini-*"]
    }

    #[instrument(skip(self, req), fields(model = %self.model_name))]
    async fn generate_content(&self, req: LlmRequest) -> Result<LlmResponse> {
        let url = self.endpoint("generateContent");
        let body = serde_json::to_vec(&to_wire(&req))?;
        debug!(bytes = body.len(), %url, "Gemini request");
        let key = self.auth_header()?;
        let resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", key)
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
            let body = String::from_utf8_lossy(&bytes).to_string();
            return Err(Error::Provider(ProviderError::Http {
                status: status.as_u16(),
                body,
            }));
        }
        parse_response(&bytes).map_err(|e| Error::Provider(ProviderError::Decode(format!("{e}"))))
    }

    async fn stream_generate_content(&self, req: LlmRequest) -> Result<LlmResponseStream> {
        let url = format!("{}?alt=sse", self.endpoint("streamGenerateContent"));
        let body = serde_json::to_vec(&to_wire(&req))?;
        let key = self.auth_header()?;
        let resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", key)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
            return Err(Error::Provider(ProviderError::Http { status, body }));
        }
        Ok(from_sse(resp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn generate_content_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
            .and(header("x-goog-api-key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [{"text": "ok"}]},
                    "finishReason": "STOP"
                }],
                "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1, "totalTokenCount": 2}
            })))
            .mount(&server)
            .await;
        let g = Gemini::new(
            "gemini-2.5-flash",
            GeminiConfig {
                base_url: server.uri(),
                api_key: "test-key".into(),
                ..GeminiConfig::default()
            },
        )
        .unwrap();
        let req = LlmRequest {
            contents: vec![crate::genai_types::Content::user_text("hi")],
            ..Default::default()
        };
        let r = g.generate_content(req).await.unwrap();
        assert_eq!(r.content.unwrap().text_concat(), "ok");
        let usage = r.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(1));
    }

    #[tokio::test]
    async fn http_error_surfaces_as_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&server)
            .await;
        let g = Gemini::new(
            "gemini-2.5-flash",
            GeminiConfig {
                base_url: server.uri(),
                api_key: "k".into(),
                ..GeminiConfig::default()
            },
        )
        .unwrap();
        let err = g.generate_content(LlmRequest::default()).await.unwrap_err();
        assert!(matches!(
            err,
            Error::Provider(ProviderError::Http { status: 429, .. })
        ));
    }

    #[tokio::test]
    async fn stream_endpoint_uses_sse_query() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-2.5-flash:streamGenerateContent"))
            .and(query_param("alt", "sse"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"STOP\"}]}\n\n",
                    ),
            )
            .mount(&server)
            .await;
        let g = Gemini::new(
            "gemini-2.5-flash",
            GeminiConfig {
                base_url: server.uri(),
                api_key: "k".into(),
                ..GeminiConfig::default()
            },
        )
        .unwrap();
        let stream = g
            .stream_generate_content(LlmRequest::default())
            .await
            .unwrap();
        let chunks = crate::providers::gemini::stream::collect_stream(stream).await.unwrap();
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].content.as_ref().unwrap().text_concat(), "hi");
    }
}
