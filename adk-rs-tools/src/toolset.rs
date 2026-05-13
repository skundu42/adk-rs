//! A [`Toolset`] is a lazy supplier of tools, used for dynamic sources like
//! MCP servers or OpenAPI specs that fetch tool lists at runtime.

use std::sync::Arc;

use async_trait::async_trait;

use adk_rs_core::{DynTool, ReadonlyContext};
use adk_rs_error::Result;

/// Anything that can yield tools at runtime.
#[async_trait]
pub trait Toolset: Send + Sync + std::fmt::Debug + 'static {
    /// Return the current set of tools.
    async fn list_tools(&self, ctx: &ReadonlyContext) -> Result<Vec<Arc<dyn DynTool>>>;

    /// Optional graceful shutdown for backends that hold OS resources.
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// Trivial toolset over a fixed `Vec<Arc<dyn DynTool>>`.
#[derive(Debug, Default, Clone)]
pub struct StaticToolset {
    tools: Vec<Arc<dyn DynTool>>,
}

impl StaticToolset {
    /// Construct from a vec.
    pub fn new(tools: Vec<Arc<dyn DynTool>>) -> Self {
        Self { tools }
    }

    /// Push a tool.
    pub fn push(&mut self, t: Arc<dyn DynTool>) {
        self.tools.push(t);
    }
}

#[async_trait]
impl Toolset for StaticToolset {
    async fn list_tools(&self, _ctx: &ReadonlyContext) -> Result<Vec<Arc<dyn DynTool>>> {
        Ok(self.tools.clone())
    }
}
