//! The [`Transport`] trait abstracts over MCP transports — stdio, streamable
//! HTTP, or SSE — so [`crate::mcp::McpClient`] doesn't care how JSON-RPC
//! messages get from this process to the server.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// A transport carries JSON-RPC traffic between this process and an MCP
/// server. Each `call` is a request that expects a response keyed by id;
/// `notify` is fire-and-forget.
#[async_trait]
pub trait Transport: Send + Sync + std::fmt::Debug + 'static {
    /// Send a JSON-RPC request and await the matching response.
    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value>;

    /// Send a JSON-RPC notification (no id, no response).
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()>;
}

/// Outgoing JSON-RPC request shape. Public to the `mcp` module only.
#[derive(Debug, Serialize)]
pub(crate) struct JsonRpcReq<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Incoming JSON-RPC envelope (response or notification).
#[derive(Debug, Deserialize)]
pub(crate) struct JsonRpcEnvelope {
    pub id: Option<u64>,
    pub method: Option<String>,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}
