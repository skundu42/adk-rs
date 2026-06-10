//! Gemini HTTP client.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use reqwest::Client;
use tracing::{debug, instrument, warn};

use crate::core::cache::{CacheMetadata, ContextCacheConfig};
use crate::core::stream::LlmResponseStream;
use crate::core::{LlmRequest, LlmResponse, Model};
use crate::error::{Error, ProviderError, Result};

use crate::providers::gemini::convert::{
    WireCachedContentCreate, parse_response, to_wire, to_wire_cached,
};
use crate::providers::gemini::stream::from_sse;

/// One explicit-cache slot, keyed by the fingerprint of the cacheable
/// prefix (model + system instruction + tools).
#[derive(Debug, Clone)]
enum CacheSlot {
    /// A live server-side cache entry.
    Active {
        name: String,
        expires_at: Instant,
        uses: u32,
    },
    /// Caching was attempted and failed (e.g. prefix below the server-side
    /// minimum); don't retry until this deadline.
    Disabled { until: Instant },
}

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
    /// Explicit context-cache entries, keyed by prefix fingerprint. Only
    /// consulted when a request carries a
    /// [`crate::core::ContextCacheConfig`].
    caches: Arc<Mutex<HashMap<u64, CacheSlot>>>,
}

impl Gemini {
    /// Construct from config and a model name.
    pub fn new(model_name: impl Into<String>, cfg: GeminiConfig) -> Result<Self> {
        // Refuse to ship an API key over plaintext HTTP. Loopback is allowed
        // for local mocks (the test suite below depends on this).
        crate::transport_security::require_secure_url(&cfg.base_url, "GeminiConfig.base_url")?;
        let http = Client::builder()
            .timeout(cfg.timeout)
            .user_agent(concat!("adk-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        Ok(Self {
            model_name: model_name.into(),
            cfg,
            http,
            caches: Arc::new(Mutex::new(HashMap::new())),
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

    /// Fingerprint of the cacheable prefix: model + system instruction +
    /// tool declarations. Any change invalidates the cache entry.
    fn cache_fingerprint(&self, req: &LlmRequest) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.model_name.hash(&mut h);
        serde_json::to_string(&req.config.system_instruction)
            .unwrap_or_default()
            .hash(&mut h);
        serde_json::to_string(&req.config.tools)
            .unwrap_or_default()
            .hash(&mut h);
        h.finish()
    }

    /// Rough token estimate of the cacheable prefix (chars / 4).
    fn estimate_prefix_tokens(req: &LlmRequest) -> u64 {
        let sys = serde_json::to_string(&req.config.system_instruction).unwrap_or_default();
        let tools = serde_json::to_string(&req.config.tools).unwrap_or_default();
        ((sys.len() + tools.len()) / 4) as u64
    }

    /// Resolve (or create) the explicit cache entry for `req`. Returns the
    /// cache resource name and whether it was a hit, or `None` when caching
    /// is skipped (too small, disabled after a failure, or creation failed).
    async fn resolve_cache(
        &self,
        req: &LlmRequest,
        cfg: &ContextCacheConfig,
    ) -> Option<CacheMetadata> {
        if req.config.system_instruction.is_none() && req.config.tools.is_empty() {
            return None;
        }
        if Self::estimate_prefix_tokens(req) < cfg.min_tokens {
            return None;
        }
        let fp = self.cache_fingerprint(req);
        {
            let mut guard = self.caches.lock();
            match guard.get_mut(&fp) {
                Some(CacheSlot::Active {
                    name,
                    expires_at,
                    uses,
                }) if *expires_at > Instant::now() && *uses < cfg.cache_intervals => {
                    *uses += 1;
                    return Some(CacheMetadata {
                        cache_name: name.clone(),
                        cache_hit: true,
                    });
                }
                Some(CacheSlot::Disabled { until }) if *until > Instant::now() => {
                    return None;
                }
                _ => {}
            }
        }

        // Create (or refresh) the entry. Failures disable caching for one
        // TTL rather than failing the call — caching is an optimization.
        match self.create_cached_content(req, cfg).await {
            Ok(name) => {
                self.caches.lock().insert(
                    fp,
                    CacheSlot::Active {
                        name: name.clone(),
                        expires_at: Instant::now() + Duration::from_secs(cfg.ttl_seconds),
                        uses: 1,
                    },
                );
                Some(CacheMetadata {
                    cache_name: name,
                    cache_hit: false,
                })
            }
            Err(e) => {
                warn!("context-cache creation failed (caching disabled for ttl): {e}");
                self.caches.lock().insert(
                    fp,
                    CacheSlot::Disabled {
                        until: Instant::now() + Duration::from_secs(cfg.ttl_seconds),
                    },
                );
                None
            }
        }
    }

    /// `POST /{version}/cachedContents` — create an explicit cache entry for
    /// the request's prefix. Returns the resource name.
    async fn create_cached_content(
        &self,
        req: &LlmRequest,
        cfg: &ContextCacheConfig,
    ) -> Result<String> {
        let url = format!(
            "{}/{}/cachedContents",
            self.cfg.base_url.trim_end_matches('/'),
            self.cfg.api_version,
        );
        let body = WireCachedContentCreate {
            model: format!("models/{}", self.model_name),
            system_instruction: req.config.system_instruction.as_ref(),
            tools: &req.config.tools,
            ttl: format!("{}s", cfg.ttl_seconds),
        };
        let key = self.auth_header()?;
        let resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", key)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body)?)
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
        let v: serde_json::Value = serde_json::from_slice(&bytes)?;
        v.get("name")
            .and_then(|n| n.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                Error::Provider(ProviderError::Decode(
                    "cachedContents response missing `name`".into(),
                ))
            })
    }

    /// Drop the cache entry that produced `cache_name` (e.g. after the
    /// server rejected it) so the next call re-creates it.
    fn invalidate_cache_entry(&self, cache_name: &str) {
        self.caches.lock().retain(
            |_, slot| !matches!(slot, CacheSlot::Active { name, .. } if name == cache_name),
        );
    }

    /// Serialize `req` to a wire body, consulting the explicit cache when a
    /// cache config is attached. Returns the body and the cache metadata.
    async fn wire_body(&self, req: &LlmRequest) -> Result<(Vec<u8>, Option<CacheMetadata>)> {
        if let Some(cfg) = &req.cache_config {
            if let Some(meta) = self.resolve_cache(req, cfg).await {
                let body = serde_json::to_vec(&to_wire_cached(req, &meta.cache_name))?;
                return Ok((body, Some(meta)));
            }
        }
        Ok((serde_json::to_vec(&to_wire(req))?, None))
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
        let (body, mut cache_meta) = self.wire_body(&req).await?;
        debug!(bytes = body.len(), %url, cached = cache_meta.is_some(), "Gemini request");
        let key = self.auth_header()?;
        let mut resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", key.clone())
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        let mut status = resp.status();

        // If the cached reference was rejected (expired/evicted server-side),
        // invalidate and retry once without the cache.
        if !status.is_success() && status.is_client_error() {
            if let Some(meta) = cache_meta.take() {
                warn!(
                    status = status.as_u16(),
                    cache = %meta.cache_name,
                    "cached request rejected; retrying without cache"
                );
                self.invalidate_cache_entry(&meta.cache_name);
                let body = serde_json::to_vec(&to_wire(&req))?;
                resp = self
                    .http
                    .post(&url)
                    .header("x-goog-api-key", key)
                    .header("content-type", "application/json")
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| ProviderError::Transport(e.to_string()))?;
                status = resp.status();
            }
        }

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
        let mut response = parse_response(&bytes)
            .map_err(|e| Error::Provider(ProviderError::Decode(format!("{e}"))))?;
        response.cache_metadata = cache_meta;
        Ok(response)
    }

    async fn stream_generate_content(&self, req: LlmRequest) -> Result<LlmResponseStream> {
        let url = format!("{}?alt=sse", self.endpoint("streamGenerateContent"));
        let (body, mut cache_meta) = self.wire_body(&req).await?;
        let key = self.auth_header()?;
        let mut resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", key.clone())
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        if !resp.status().is_success() && resp.status().is_client_error() {
            if let Some(meta) = cache_meta.take() {
                warn!(
                    status = resp.status().as_u16(),
                    cache = %meta.cache_name,
                    "cached stream request rejected; retrying without cache"
                );
                self.invalidate_cache_entry(&meta.cache_name);
                let body = serde_json::to_vec(&to_wire(&req))?;
                resp = self
                    .http
                    .post(&url)
                    .header("x-goog-api-key", key)
                    .header("content-type", "application/json")
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| ProviderError::Transport(e.to_string()))?;
            }
        }
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
    async fn rejects_plaintext_http_base_url() {
        let err = Gemini::new(
            "gemini-2.5-flash",
            GeminiConfig {
                base_url: "http://generativelanguage.googleapis.com".into(),
                api_key: "k".into(),
                ..GeminiConfig::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("https"));
    }

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
    async fn context_cache_created_once_and_reused() {
        use wiremock::matchers::body_partial_json;

        let server = MockServer::start().await;
        // Cache creation must happen exactly once.
        Mock::given(method("POST"))
            .and(path("/v1beta/cachedContents"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"name": "cachedContents/abc123"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        // Cached generate calls reference the entry and omit the prefix.
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
            .and(body_partial_json(
                json!({"cachedContent": "cachedContents/abc123"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [{"text": "ok"}]},
                    "finishReason": "STOP"
                }]
            })))
            .expect(2)
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
        let mut req = LlmRequest {
            contents: vec![crate::genai_types::Content::user_text("hi")],
            cache_config: Some(ContextCacheConfig::default()),
            ..Default::default()
        };
        req.append_system_text("a long stable instruction");

        let r1 = g.generate_content(req.clone()).await.unwrap();
        let m1 = r1.cache_metadata.unwrap();
        assert_eq!(m1.cache_name, "cachedContents/abc123");
        assert!(!m1.cache_hit, "first call creates the entry");

        let r2 = g.generate_content(req).await.unwrap();
        let m2 = r2.cache_metadata.unwrap();
        assert!(m2.cache_hit, "second call reuses the entry");
        // Mock expectations (1 create, 2 cached generates) assert the rest.
    }

    #[tokio::test]
    async fn context_cache_skipped_below_min_tokens() {
        let server = MockServer::start().await;
        // No cachedContents call expected at all.
        Mock::given(method("POST"))
            .and(path("/v1beta/cachedContents"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [{"text": "ok"}]},
                    "finishReason": "STOP"
                }]
            })))
            .expect(1)
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
        let mut req = LlmRequest {
            cache_config: Some(ContextCacheConfig {
                min_tokens: 100_000,
                ..ContextCacheConfig::default()
            }),
            ..Default::default()
        };
        req.append_system_text("tiny");
        let r = g.generate_content(req).await.unwrap();
        assert!(r.cache_metadata.is_none());
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
        let chunks = crate::providers::gemini::stream::collect_stream(stream)
            .await
            .unwrap();
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].content.as_ref().unwrap().text_concat(), "hi");
    }
}
