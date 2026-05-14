//! [`McpClient`] — a thin facade over an [`Transport`] that knows how to
//! talk MCP (initialize handshake, `tools/list`, `tools/call`).

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use crate::error::Result;
use crate::mcp::http::{HttpTransport, McpHttpParams};
use crate::mcp::stdio::{McpStdioParams, StdioTransport};
use crate::mcp::transport::Transport;

/// MCP JSON-RPC client.
pub struct McpClient {
    transport: Arc<dyn Transport>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("transport", &self.transport)
            .finish()
    }
}

impl McpClient {
    /// Construct from a pre-built transport. Performs the MCP `initialize`
    /// handshake before returning.
    pub async fn connect(transport: Arc<dyn Transport>) -> Result<Self> {
        let me = Self { transport };
        me.initialize().await?;
        Ok(me)
    }

    /// Spawn an MCP server as a child process over stdio. Most common form.
    pub async fn spawn(params: McpStdioParams) -> Result<Self> {
        let t = Arc::new(StdioTransport::spawn(params).await?) as Arc<dyn Transport>;
        Self::connect(t).await
    }

    /// Connect to a remote MCP server over streamable HTTP.
    pub async fn http(params: McpHttpParams) -> Result<Self> {
        let t = Arc::new(HttpTransport::new(params)?) as Arc<dyn Transport>;
        Self::connect(t).await
    }

    async fn initialize(&self) -> Result<()> {
        let init = self
            .transport
            .call(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "clientInfo": {"name": "adk-rs", "version": env!("CARGO_PKG_VERSION")},
                })),
            )
            .await?;
        debug!(?init, "MCP initialized");
        self.transport
            .notify("notifications/initialized", None)
            .await?;
        Ok(())
    }

    /// Send a JSON-RPC request and await the response.
    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.transport.call(method, params).await
    }

    /// Send a JSON-RPC notification (no response expected).
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        self.transport.notify(method, params).await
    }

    /// List tools advertised by the server.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
        let v = self.transport.call("tools/list", None).await?;
        #[derive(Deserialize)]
        struct R {
            tools: Vec<McpToolDescriptor>,
        }
        let r: R = serde_json::from_value(v)?;
        Ok(r.tools)
    }

    /// Call a tool by name.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value> {
        self.transport
            .call(
                "tools/call",
                Some(serde_json::json!({"name": name, "arguments": args})),
            )
            .await
    }
}

/// One advertised MCP tool.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDescriptor {
    /// Tool name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// JSON-schema describing the args.
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn tool_descriptor_round_trip() {
        let payload =
            r#"{"name":"weather","description":"look up weather","inputSchema":{"type":"object"}}"#;
        let d: McpToolDescriptor = serde_json::from_str(payload).unwrap();
        assert_eq!(d.name, "weather");
        assert_eq!(d.description, "look up weather");
        assert!(d.input_schema.is_some());
    }

    #[tokio::test]
    async fn http_client_end_to_end_lists_and_calls_tool() {
        let server = MockServer::start().await;
        // initialize → result
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(json!({"method": "initialize"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc":"2.0","id":1,
                "result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}}}
            })))
            .mount(&server)
            .await;
        // notifications/initialized → 202 with no body would be ideal,
        // but wiremock's default empty body works as a 200 too.
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(
                json!({"method": "notifications/initialized"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        // tools/list
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc":"2.0","id":2,
                "result":{"tools":[
                    {"name":"echo","description":"echo back","inputSchema":{"type":"object"}}
                ]}
            })))
            .mount(&server)
            .await;
        // tools/call
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(json!({"method": "tools/call"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc":"2.0","id":3,
                "result":{"content":[{"type":"text","text":"hello"}]}
            })))
            .mount(&server)
            .await;

        let client = McpClient::http(crate::mcp::http::McpHttpParams {
            url: format!("{}/mcp", server.uri()),
            timeout: Duration::from_secs(5),
            ..crate::mcp::http::McpHttpParams::default()
        })
        .await
        .unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        let r = client
            .call_tool("echo", json!({"msg": "hi"}))
            .await
            .unwrap();
        assert_eq!(r["content"][0]["text"], "hello");
    }
}
