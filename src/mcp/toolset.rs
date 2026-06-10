//! [`McpToolset`] — a [`Toolset`] that owns an [`McpClient`] and exposes
//! discovered tools.

use std::sync::Arc;

use async_trait::async_trait;

use crate::core::{DynTool, ReadonlyContext};
use crate::error::Result;
use crate::tools::Toolset;

use crate::mcp::client::McpClient;
use crate::mcp::http::McpHttpParams;
use crate::mcp::stdio::McpStdioParams;
use crate::mcp::tool::McpTool;

/// Which discovered tools require user confirmation before each call.
#[derive(Debug, Clone, Default)]
pub enum ConfirmationPolicy {
    /// No tool requires confirmation (default).
    #[default]
    None,
    /// Every tool requires confirmation.
    All,
    /// Only the named tools require confirmation.
    Named(std::collections::HashSet<String>),
}

impl ConfirmationPolicy {
    fn applies_to(&self, name: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Named(set) => set.contains(name),
        }
    }
}

/// MCP-backed toolset.
pub struct McpToolset {
    client: Arc<McpClient>,
    confirmation: ConfirmationPolicy,
    cached: tokio::sync::OnceCell<Vec<Arc<dyn DynTool>>>,
}

impl std::fmt::Debug for McpToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolset").finish_non_exhaustive()
    }
}

impl McpToolset {
    /// Wrap an already-connected client.
    pub fn from_client(client: Arc<McpClient>) -> Self {
        Self {
            client,
            confirmation: ConfirmationPolicy::default(),
            cached: tokio::sync::OnceCell::new(),
        }
    }

    /// Require user confirmation before tool calls (human-in-the-loop).
    /// Use [`ConfirmationPolicy::All`] for every discovered tool or
    /// [`ConfirmationPolicy::Named`] for a subset. Must be set before the
    /// first `list_tools` call (discovered tools are cached).
    #[must_use]
    pub fn with_confirmation_policy(mut self, policy: ConfirmationPolicy) -> Self {
        self.confirmation = policy;
        self
    }

    /// Spawn an MCP server over stdio.
    pub async fn stdio(params: McpStdioParams) -> Result<Self> {
        Ok(Self::from_client(Arc::new(McpClient::spawn(params).await?)))
    }

    /// Connect to a remote MCP server over streamable HTTP.
    pub async fn http(params: McpHttpParams) -> Result<Self> {
        Ok(Self::from_client(Arc::new(McpClient::http(params).await?)))
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
            .map(|d| {
                let confirm = self.confirmation.applies_to(&d.name);
                Arc::new(McpTool::new(d, self.client.clone()).with_require_confirmation(confirm))
                    as Arc<dyn DynTool>
            })
            .collect();
        let _ = self.cached.set(tools.clone());
        Ok(tools)
    }
}
