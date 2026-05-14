//! Stdio JSON-RPC transport. Spawns the MCP server as a child process and
//! talks newline-delimited JSON over stdin/stdout.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as TokioMutex, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::mcp::transport::{JsonRpcEnvelope, JsonRpcReq, Transport};

/// Spawn parameters for the stdio transport.
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

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<core::result::Result<Value, String>>>>>;

/// Stdio JSON-RPC transport.
pub struct StdioTransport {
    next_id: Arc<Mutex<u64>>,
    pending: Pending,
    writer: Arc<TokioMutex<ChildStdin>>,
    _reader_task: JoinHandle<()>,
    _child: Arc<Mutex<Option<Child>>>,
    timeout: Duration,
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport").finish_non_exhaustive()
    }
}

impl StdioTransport {
    /// Spawn the server and connect.
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
                                    Some(e) => Err(e.to_string()),
                                    None => Ok(env.result.unwrap_or(Value::Null)),
                                };
                                let _ = tx.send(result);
                            }
                        } else if let Some(m) = env.method {
                            debug!(method = %m, "MCP notification");
                        }
                    }
                    Err(e) => {
                        warn!("MCP malformed line: {e}; line={line}");
                    }
                }
            }
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::warn!(target: "adk::mcp::child", "{line}");
            }
        });

        Ok(Self {
            next_id: Arc::new(Mutex::new(1)),
            pending,
            writer: Arc::new(TokioMutex::new(stdin)),
            _reader_task: reader_task,
            _child: Arc::new(Mutex::new(Some(child))),
            timeout: params.timeout,
        })
    }

    fn allocate_id(&self) -> u64 {
        let mut g = self.next_id.lock();
        let id = *g;
        *g += 1;
        id
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
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

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_rejects_empty_command() {
        let err = StdioTransport::spawn(McpStdioParams::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("command is empty"));
    }

    #[tokio::test]
    async fn spawn_reports_missing_binary() {
        let params = McpStdioParams {
            command: "definitely-not-a-real-binary-adkrs".into(),
            ..McpStdioParams::default()
        };
        let err = StdioTransport::spawn(params).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("spawn"));
    }
}
