//! `load_memory` built-in. Queries the configured
//! [`MemoryService`](crate::core::MemoryService) and returns matching entries.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, ToolContext};
use crate::error::{Error, Result};
use crate::genai_types::{FunctionDeclaration, Schema};

/// Look up previously-seen sessions / memory entries that match `query`.
#[derive(Debug)]
struct LoadMemory;

#[async_trait]
impl DynTool for LoadMemory {
    fn name(&self) -> &str {
        "load_memory"
    }
    fn description(&self) -> &str {
        "Search long-term memory for entries that match the given query and \
         return them so they can be used in the response."
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(
            FunctionDeclaration::new(self.name(), self.description()).with_parameters(
                Schema::object()
                    .property(
                        "query",
                        Schema::string().with_description("Free-text query."),
                    )
                    .require("query"),
            ),
        )
    }
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid_input("query must be a string"))?;
        let svc = ctx
            .invocation
            .memory_service
            .as_ref()
            .ok_or_else(|| Error::config("no memory service configured"))?
            .clone();
        let app = ctx.invocation.app_name.clone();
        let user = ctx.invocation.user_id.clone();
        let resp = svc.search_memory(&app, &user, query).await?;
        Ok(serde_json::json!({
            "memories": resp.memories,
        }))
    }
}

/// Construct the `load_memory` tool.
#[must_use]
pub fn load_memory_tool() -> Arc<dyn DynTool> {
    Arc::new(LoadMemory)
}
