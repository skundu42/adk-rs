//! A single MCP-backed [`crate::core::DynTool`].

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::warn;

use crate::core::{DynTool, ToolContext};
use crate::error::Result;
use crate::genai_types::{FunctionDeclaration, Schema};

use crate::mcp::client::{McpClient, McpToolDescriptor};

/// A tool that proxies to an MCP server.
#[derive(Clone)]
pub struct McpTool {
    name: String,
    description: String,
    schema: Option<Schema>,
    client: Arc<McpClient>,
    require_confirmation: bool,
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTool").field("name", &self.name).finish()
    }
}

impl McpTool {
    /// Construct from a descriptor and a client handle.
    pub fn new(d: McpToolDescriptor, client: Arc<McpClient>) -> Self {
        let schema =
            d.input_schema
                .and_then(|v| match serde_json::from_value::<Schema>(v.clone()) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        warn!(
                            tool = %d.name,
                            "MCP input_schema didn't parse as adk Schema: {e}; raw was: {v}"
                        );
                        None
                    }
                });
        Self {
            name: d.name,
            description: d.description,
            schema,
            client,
            require_confirmation: false,
        }
    }

    /// Require explicit user confirmation before each call.
    #[must_use]
    pub fn with_require_confirmation(mut self, yes: bool) -> Self {
        self.require_confirmation = yes;
        self
    }
}

#[async_trait]
impl DynTool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn requires_confirmation(&self, _args: &Value) -> bool {
        self.require_confirmation
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(
            FunctionDeclaration::new(&self.name, &self.description)
                .with_parameters(self.schema.clone().unwrap_or_else(Schema::object)),
        )
    }
    async fn run(&self, args: Value, _ctx: &mut ToolContext) -> Result<Value> {
        self.client.call_tool(&self.name, args).await
    }
}
