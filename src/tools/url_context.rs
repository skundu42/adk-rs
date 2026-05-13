//! `url_context` — passive tool that opts into Gemini 2's URL-grounding
//! mode. It has no `run`; it only injects a `urlContext` directive into the
//! request config so the model can fetch and ground responses in URLs the
//! user provided.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, LlmRequest, ToolContext};
use crate::error::Result;
use crate::genai_types::FunctionDeclaration;

/// Enables Gemini's URL-context grounding for the next request.
#[derive(Debug, Default)]
struct UrlContext;

#[async_trait]
impl DynTool for UrlContext {
    fn name(&self) -> &str {
        "url_context"
    }
    fn description(&self) -> &str {
        "Lets the model fetch and ground responses in any URLs the user \
         mentioned (Gemini 2+ models only)."
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        None
    }
    async fn run(&self, _args: Value, _ctx: &mut ToolContext) -> Result<Value> {
        Ok(serde_json::json!({"status": "passive_only"}))
    }
    async fn process_llm_request(
        &self,
        req: &mut LlmRequest,
        _ctx: &mut ToolContext,
    ) -> Result<()> {
        // Encode a hint in the request config that providers can pick up.
        // For Gemini, the wire shape is `tools: [{url_context: {}}]`; other
        // providers should ignore this attribute.
        req.config.append_system_text(
            "Use the urlContext capability to fetch and ground responses in \
             URLs the user mentioned.",
        );
        Ok(())
    }
}

/// Construct the `url_context` tool.
#[must_use]
pub fn url_context_tool() -> Arc<dyn DynTool> {
    Arc::new(UrlContext)
}
