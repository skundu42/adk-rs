//! `preload_memory` — a "passive" tool that adds memory excerpts to the
//! system prompt at turn start. It has no callable `run`; it only implements
//! [`DynTool::process_llm_request`].

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, LlmRequest, ToolContext};
use crate::error::Result;
use crate::genai_types::FunctionDeclaration;

/// Pre-populates the LLM request with memory entries relevant to the user
/// input. Activated only when both a memory service is configured and the
/// invocation has user content to query with.
#[derive(Debug)]
struct PreloadMemory {
    /// Hard cap on the number of memory entries inlined per turn.
    max_entries: usize,
}

#[async_trait]
impl DynTool for PreloadMemory {
    fn name(&self) -> &str {
        "preload_memory"
    }
    fn description(&self) -> &str {
        "Internal: pre-loads memory entries into the LLM request when the \
         user input matches stored memories."
    }
    /// Not advertised to the model — passive only.
    fn declaration(&self) -> Option<FunctionDeclaration> {
        None
    }
    async fn run(&self, _args: Value, _ctx: &mut ToolContext) -> Result<Value> {
        Ok(serde_json::json!({"status": "passive_only"}))
    }

    async fn process_llm_request(&self, req: &mut LlmRequest, ctx: &mut ToolContext) -> Result<()> {
        let svc = match ctx.invocation.memory_service.as_ref() {
            Some(s) => s.clone(),
            None => return Ok(()),
        };
        let query = match ctx.invocation.user_content.as_ref() {
            Some(c) => c.text_concat(),
            None => return Ok(()),
        };
        if query.trim().is_empty() {
            return Ok(());
        }
        let resp = svc
            .search_memory(&ctx.invocation.app_name, &ctx.invocation.user_id, &query)
            .await?;
        if resp.memories.is_empty() {
            return Ok(());
        }
        let mut buf = String::from("\n\nRelevant prior context:\n");
        for entry in resp.memories.iter().take(self.max_entries) {
            buf.push_str("- ");
            buf.push_str(&entry.content.text_concat());
            buf.push('\n');
        }
        req.config.append_system_text(&buf);
        Ok(())
    }
}

/// Construct the `preload_memory` tool. The `max_entries` value bounds the
/// number of memory hits inlined into the system prompt each turn.
#[must_use]
pub fn preload_memory_tool(max_entries: usize) -> Arc<dyn DynTool> {
    Arc::new(PreloadMemory { max_entries })
}
