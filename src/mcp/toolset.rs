//! [`McpToolset`] — a [`Toolset`] that owns an [`McpClient`] and exposes
//! discovered tools.

use std::sync::Arc;

use async_trait::async_trait;

use crate::core::{DynTool, ReadonlyContext};
use crate::error::Result;
use crate::tools::Toolset;

use crate::mcp::client::{McpClient, McpStdioParams};
use crate::mcp::tool::McpTool;

/// MCP-backed toolset.
pub struct McpToolset {
    client: Arc<McpClient>,
    cached: tokio::sync::OnceCell<Vec<Arc<dyn DynTool>>>,
}

impl std::fmt::Debug for McpToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolset").finish_non_exhaustive()
    }
}

impl McpToolset {
    /// Spawn an MCP server over stdio.
    pub async fn stdio(params: McpStdioParams) -> Result<Self> {
        let client = Arc::new(McpClient::spawn(params).await?);
        Ok(Self {
            client,
            cached: tokio::sync::OnceCell::new(),
        })
    }
}

#[async_trait]
impl Toolset for McpToolset {
    async fn list_tools(&self, _ctx: &ReadonlyContext) -> Result<Vec<Arc<dyn DynTool>>> {
        if let Some(t) = self.cached.get() {
            return Ok(t.clone());
        }
        let descs = self.client.list_tools().await?;
        let tools: Vec<Arc<dyn DynTool>> = descs
            .into_iter()
            .map(|d| Arc::new(McpTool::new(d, self.client.clone())) as Arc<dyn DynTool>)
            .collect();
        let _ = self.cached.set(tools.clone());
        Ok(tools)
    }
}
