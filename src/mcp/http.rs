//! Streamable-HTTP JSON-RPC transport for MCP servers.
//!
//! Follows the MCP Streamable HTTP profile: a single POST endpoint that
//! accepts a JSON-RPC request and returns either an `application/json`
//! response (single result) or a `text/event-stream` (SSE) where each
//! `message` event carries a JSON-RPC response. Server-issued
//! `Mcp-Session-Id` headers are remembered and echoed on subsequent calls.
//!
//! Legacy SSE servers — those that require a separate GET endpoint to open
//! the event channel and a different POST endpoint for messages — are not
//! supported here; the streamable-HTTP profile has subsumed that pattern in
//! the MCP 2025-03 spec onward.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use parking_lot::Mutex;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use tracing::debug;

use crate::error::{Error, Result};
use crate::mcp::transport::{JsonRpcEnvelope, JsonRpcReq, Transport};

/// Configuration for the HTTP transport.
#[derive(Debug, Clone)]
pub struct McpHttpParams {
    /// MCP server endpoint URL (e.g. `https://example.com/mcp`).
    pub url: String,
    /// Extra request headers (e.g. `Authorization: Bearer ...`).
    pub headers: HashMap<String, String>,
    /// Per-call timeout.
    pub timeout: Duration,
}

impl Default for McpHttpParams {
    fn default() -> Self {
        Self {
            url: String::new(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(60),
        }
    }
}

const MCP_SESSION_HEADER: &str = "mcp-session-id";

/// Streamable-HTTP JSON-RPC transport.
pub struct HttpTransport {
    http: Client,
    url: String,
    extra_headers: HeaderMap,
    session_id: Arc<Mutex<Option<String>>>,
    next_id: Arc<Mutex<u64>>,
    timeout: Duration,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl HttpTransport {
    /// Construct a new HTTP transport. Does not perform the MCP `initialize`
    /// handshake — that is driven by [`crate::mcp::McpClient`].
    ///
    /// If the caller supplies any credential-bearing header (`Authorization`,
    /// `Cookie`, `Proxy-Authorization`, or any header beginning with `x-api`
    /// / `x-auth`), the URL must be HTTPS or loopback — otherwise the
    /// secret would travel in plaintext over the wire.
    pub fn new(params: McpHttpParams) -> Result<Self> {
        if params.url.is_empty() {
            return Err(Error::config("McpHttpParams.url is empty"));
        }
        if params
            .headers
            .keys()
            .any(|k| header_looks_credential_bearing(k.as_str()))
        {
            crate::transport_security::require_secure_url(&params.url, "McpHttpParams.url")?;
        }
        let mut headers = HeaderMap::new();
        for (k, v) in &params.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| Error::config(format!("invalid MCP header {k}: {e}")))?;
            let value = HeaderValue::from_str(v)
                .map_err(|e| Error::config(format!("invalid MCP header value: {e}")))?;
            headers.insert(name, value);
        }
        let http = Client::builder()
            .timeout(params.timeout)
            .user_agent(concat!("adk-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::other(format!("MCP HTTP client: {e}")))?;
        Ok(Self {
            http,
            url: params.url,
            extra_headers: headers,
            session_id: Arc::new(Mutex::new(None)),
            next_id: Arc::new(Mutex::new(1)),
            timeout: params.timeout,
        })
    }

    fn allocate_id(&self) -> u64 {
        let mut g = self.next_id.lock();
        let id = *g;
        *g += 1;
        id
    }

    fn build_headers(&self) -> HeaderMap {
        let mut h = self.extra_headers.clone();
        h.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(sid) = self.session_id.lock().as_ref() {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(MCP_SESSION_HEADER.as_bytes()),
                HeaderValue::from_str(sid),
            ) {
                h.insert(name, value);
            }
        }
        h
    }

    fn capture_session(&self, resp: &reqwest::Response) {
        if let Some(v) = resp.headers().get(MCP_SESSION_HEADER) {
            if let Ok(s) = v.to_str() {
                *self.session_id.lock() = Some(s.to_string());
            }
        }
    }

    /// Send a JSON-RPC envelope, returning the parsed result for the supplied
    /// `expected_id`. Notifications go through [`Self::send_notification`].
    async fn send_request(&self, id: u64, body: Vec<u8>) -> Result<Value> {
        let resp = self
            .http
            .post(&self.url)
            .headers(self.build_headers())
            .body(body)
            .send()
            .await
            .map_err(|e| Error::other(format!("MCP HTTP request: {e}")))?;
        self.capture_session(&resp);
        let status = resp.status();
        if status == StatusCode::ACCEPTED && resp.content_length() == Some(0) {
            return Err(Error::other(
                "MCP server returned 202 with no body for a request expecting a response",
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::other(format!("MCP HTTP error ({status}): {body}")));
        }
        let ctype = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ctype.starts_with("text/event-stream") {
            self.consume_sse(resp, id).await
        } else {
            // application/json — single response, possibly batched.
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| Error::other(format!("MCP HTTP body: {e}")))?;
            parse_match(&bytes, id)
        }
    }

    async fn consume_sse(&self, resp: reqwest::Response, id: u64) -> Result<Value> {
        let stream = resp.bytes_stream().eventsource();
        tokio::pin!(stream);
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let next = tokio::time::timeout_at(deadline, stream.next()).await;
            match next {
                Err(_) => return Err(Error::other("MCP SSE call timed out")),
                Ok(None) => return Err(Error::other("MCP SSE closed before response arrived")),
                Ok(Some(Err(e))) => return Err(Error::other(format!("MCP SSE: {e}"))),
                Ok(Some(Ok(event))) => {
                    let data = event.data.trim();
                    if data.is_empty() {
                        continue;
                    }
                    debug!(?event.event, %data, "MCP SSE chunk");
                    let env = match serde_json::from_str::<JsonRpcEnvelope>(data) {
                        Ok(env) => env,
                        Err(e) => {
                            return Err(Error::other(format!("MCP malformed envelope: {e}")));
                        }
                    };
                    match env.id {
                        None => {
                            if let Some(m) = env.method {
                                debug!(method = %m, "MCP HTTP notification");
                            }
                            continue;
                        }
                        Some(rid) if rid == id => {
                            if let Some(e) = env.error {
                                return Err(Error::other(e.to_string()));
                            }
                            return Ok(env.result.unwrap_or(Value::Null));
                        }
                        Some(rid) => {
                            debug!(rid, "MCP HTTP response for unexpected id");
                            continue;
                        }
                    }
                }
            }
        }
    }

    async fn send_notification(&self, body: Vec<u8>) -> Result<()> {
        let resp = self
            .http
            .post(&self.url)
            .headers(self.build_headers())
            .body(body)
            .send()
            .await
            .map_err(|e| Error::other(format!("MCP HTTP notify: {e}")))?;
        self.capture_session(&resp);
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::other(format!(
                "MCP HTTP notify error ({status}): {body}"
            )));
        }
        Ok(())
    }
}

/// Heuristic: would shipping this header to a plaintext endpoint leak a
/// credential? Matches `Authorization`, `Cookie`, `Proxy-Authorization`,
/// and the conventional `x-api-*` / `x-auth-*` patterns case-insensitively.
fn header_looks_credential_bearing(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization" | "cookie" | "proxy-authorization"
    ) || lower.starts_with("x-api")
        || lower.starts_with("x-auth")
}

fn parse_match(bytes: &[u8], expected_id: u64) -> Result<Value> {
    let env: JsonRpcEnvelope = serde_json::from_slice(bytes)
        .map_err(|e| Error::other(format!("MCP malformed envelope: {e}")))?;
    if env.id != Some(expected_id) {
        return Err(Error::other(format!(
            "MCP response id mismatch (got {:?}, expected {expected_id})",
            env.id
        )));
    }
    if let Some(e) = env.error {
        return Err(Error::other(e.to_string()));
    }
    Ok(env.result.unwrap_or(Value::Null))
}

#[async_trait]
impl Transport for HttpTransport {
    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.allocate_id();
        let req = JsonRpcReq {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let body = serde_json::to_vec(&req)?;
        self.send_request(id, body).await
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let v = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });
        let body = serde_json::to_vec(&v)?;
        self.send_notification(body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn params(url: String) -> McpHttpParams {
        McpHttpParams {
            url,
            timeout: Duration::from_secs(5),
            ..McpHttpParams::default()
        }
    }

    #[tokio::test]
    async fn rejects_empty_url() {
        let err = HttpTransport::new(McpHttpParams::default()).err().unwrap();
        assert!(err.to_string().contains("url is empty"));
    }

    #[tokio::test]
    async fn rejects_auth_header_over_plaintext_http() {
        let mut p = McpHttpParams {
            url: "http://example.com/mcp".into(),
            timeout: Duration::from_secs(5),
            ..McpHttpParams::default()
        };
        p.headers
            .insert("Authorization".into(), "Bearer secret".into());
        let err = HttpTransport::new(p).err().unwrap();
        assert!(err.to_string().to_lowercase().contains("https"));
    }

    #[tokio::test]
    async fn rejects_x_api_key_over_plaintext_http() {
        let mut p = McpHttpParams {
            url: "http://example.com/mcp".into(),
            timeout: Duration::from_secs(5),
            ..McpHttpParams::default()
        };
        p.headers.insert("X-Api-Key".into(), "secret".into());
        let err = HttpTransport::new(p).err().unwrap();
        assert!(err.to_string().to_lowercase().contains("https"));
    }

    #[tokio::test]
    async fn allows_authed_loopback_http() {
        let mut p = McpHttpParams {
            url: "http://127.0.0.1:8765/mcp".into(),
            timeout: Duration::from_secs(5),
            ..McpHttpParams::default()
        };
        p.headers.insert("Authorization".into(), "Bearer x".into());
        HttpTransport::new(p).unwrap();
    }

    #[tokio::test]
    async fn allows_unauthed_http() {
        // No credential header → plaintext is fine.
        let p = McpHttpParams {
            url: "http://example.com/mcp".into(),
            timeout: Duration::from_secs(5),
            ..McpHttpParams::default()
        };
        HttpTransport::new(p).unwrap();
    }

    #[tokio::test]
    async fn json_response_round_trip() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"tools": []}
            })))
            .mount(&server)
            .await;
        let t = HttpTransport::new(params(format!("{}/mcp", server.uri()))).unwrap();
        let v = t.call("tools/list", None).await.unwrap();
        assert_eq!(v, json!({"tools": []}));
    }

    #[tokio::test]
    async fn sends_extra_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("authorization", "Bearer abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc":"2.0","id":1,"result":{"ok": true}
            })))
            .mount(&server)
            .await;
        let mut p = params(format!("{}/mcp", server.uri()));
        p.headers
            .insert("Authorization".into(), "Bearer abc".into());
        let t = HttpTransport::new(p).unwrap();
        t.call("ping", None).await.unwrap();
    }

    #[tokio::test]
    async fn captures_and_echoes_session_id() {
        let server = MockServer::start().await;
        // First call → server hands back a session id.
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("mcp-session-id", "sess-42")
                    .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second call must echo the session header back.
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("mcp-session-id", "sess-42"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":2,"result":{"ok":true}})),
            )
            .mount(&server)
            .await;
        let t = HttpTransport::new(params(format!("{}/mcp", server.uri()))).unwrap();
        t.call("initialize", None).await.unwrap();
        t.call("tools/list", None).await.unwrap();
    }

    #[tokio::test]
    async fn sse_response_picks_matching_id() {
        let server = MockServer::start().await;
        // Stream a notification first, then the response.
        let sse_body = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"toolListChanged\"}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"x\"}]}}\n\n",
        );
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(sse_body.as_bytes().to_vec(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let t = HttpTransport::new(params(format!("{}/mcp", server.uri()))).unwrap();
        let v = t.call("tools/list", None).await.unwrap();
        assert_eq!(v["tools"][0]["name"], "x");
    }

    #[tokio::test]
    async fn surfaces_json_rpc_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc":"2.0","id":1,
                "error":{"code":-32601,"message":"method not found"}
            })))
            .mount(&server)
            .await;
        let t = HttpTransport::new(params(format!("{}/mcp", server.uri()))).unwrap();
        let err = t.call("tools/list", None).await.unwrap_err();
        assert!(err.to_string().contains("method not found"));
    }

    #[tokio::test]
    async fn http_5xx_surfaces() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        let t = HttpTransport::new(params(format!("{}/mcp", server.uri()))).unwrap();
        let err = t.call("ping", None).await.unwrap_err();
        assert!(err.to_string().contains("500"));
    }
}
