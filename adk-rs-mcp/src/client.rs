//! MCP stdio JSON-RPC client.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as TokioMutex, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use adk_rs_error::{Error, Result};

/// stdio spawn parameters.
#[derive(Debug, Clone)]
pub struct McpStdioParams {
    /// Command to run.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Extra env vars.
    pub env: HashMap<String, String>,
    /// Per-call timeout.
    pub timeout: Duration,
}

impl Default for McpStdioParams {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
        }
    }
}

/// Outgoing JSON-RPC request shape.
#[derive(Debug, Serialize)]
struct JsonRpcReq<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// Incoming envelope (response or notification).
#[derive(Debug, Deserialize)]
struct JsonRpcEnvelope {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<u64>,
    method: Option<String>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
    #[allow(dead_code)]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i32,
    message: String,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<core::result::Result<Value, String>>>>>;

/// MCP stdio client.
pub struct McpClient {
    next_id: Arc<Mutex<u64>>,
    pending: Pending,
    writer: Arc<TokioMutex<ChildStdin>>,
    _reader_task: JoinHandle<()>,
    _child: Arc<Mutex<Option<Child>>>,
    timeout: Duration,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient").finish_non_exhaustive()
    }
}

impl McpClient {
    /// Spawn the MCP server and connect.
    pub async fn spawn(params: McpStdioParams) -> Result<Self> {
        if params.command.is_empty() {
            return Err(Error::config("McpStdioParams.command is empty"));
        }
        let mut cmd = Command::new(&params.command);
        cmd.args(&params.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &params.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::other(format!("MCP spawn '{}' failed: {e}", params.command)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::other("MCP child stdin missing"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::other("MCP child stdout missing"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::other("MCP child stderr missing"))?;

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let pending_for_reader = pending.clone();
        let reader_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcEnvelope>(&line) {
                    Ok(env) => {
                        if let Some(id) = env.id {
                            let tx_opt = pending_for_reader.lock().remove(&id);
                            if let Some(tx) = tx_opt {
                                let result = match env.error {
                                    Some(e) => Err(e.message),
                                    None => Ok(env.result.unwrap_or(Value::Null)),
                                };
                                let _ = tx.send(result);
                            }
                        } else if let Some(m) = env.method {
                            // notifications: just log for now.
                            debug!(method = %m, "MCP notification");
                        }
                    }
                    Err(e) => {
                        warn!("MCP malformed line: {e}; line={line}");
                    }
                }
            }
        });

        // stderr drain.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::warn!(target: "adk::mcp::child", "{line}");
            }
        });

        let client = Self {
            next_id: Arc::new(Mutex::new(1)),
            pending,
            writer: Arc::new(TokioMutex::new(stdin)),
            _reader_task: reader_task,
            _child: Arc::new(Mutex::new(Some(child))),
            timeout: params.timeout,
        };

        // Initialize handshake.
        let init: Value = client
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
        client.notify("notifications/initialized", None).await?;
        Ok(client)
    }

    fn allocate_id(&self) -> u64 {
        let mut g = self.next_id.lock();
        let id = *g;
        *g += 1;
        id
    }

    /// Send a JSON-RPC request and await its response.
    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.allocate_id();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let req = JsonRpcReq {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let body = serde_json::to_vec(&req)?;
        {
            let mut w = self.writer.lock().await;
            w.write_all(&body)
                .await
                .map_err(|e| Error::other(format!("MCP write: {e}")))?;
            w.write_all(b"\n")
                .await
                .map_err(|e| Error::other(format!("MCP write: {e}")))?;
            w.flush().await.ok();
        }
        let resp = tokio::time::timeout(self.timeout, rx)
            .await
            .map_err(|_| Error::other("MCP call timed out"))?
            .map_err(|_| Error::other("MCP responder dropped"))?;
        resp.map_err(Error::other)
    }

    /// Send a JSON-RPC notification (no id, no response).
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        // Notifications: same envelope but no `id`.
        let v = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });
        let body = serde_json::to_vec(&v)?;
        let mut w = self.writer.lock().await;
        w.write_all(&body)
            .await
            .map_err(|e| Error::other(format!("MCP write: {e}")))?;
        w.write_all(b"\n")
            .await
            .map_err(|e| Error::other(format!("MCP write: {e}")))?;
        w.flush().await.ok();
        Ok(())
    }

    /// List tools advertised by the server.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
        let v = self.call("tools/list", None).await?;
        #[derive(Deserialize)]
        struct R {
            tools: Vec<McpToolDescriptor>,
        }
        let r: R = serde_json::from_value(v).map_err(Error::from)?;
        Ok(r.tools)
    }

    /// Call a tool by name.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value> {
        self.call(
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
