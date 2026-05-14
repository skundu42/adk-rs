//! `url_context` — passive tool that opts into Gemini 2's URL-grounding
//! mode. It has no `run`; it only injects a `urlContext` directive into the
//! request config so the model can fetch and ground responses in URLs the
//! user provided.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, LlmRequest, ToolContext};
use crate::error::Result;
use crate::genai_types::{FunctionDeclaration, Tool};

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
        if !req
            .config
            .tools
            .iter()
            .any(|t| matches!(t, Tool::UrlContext {}))
        {
            req.config.tools.push(Tool::UrlContext {});
        }
        Ok(())
    }
}

/// Construct the `url_context` tool. Attach it via `LlmAgent::builder().tool(...)`
/// to opt the Gemini request into URL-context grounding.
#[must_use]
pub fn url_context_tool() -> Arc<dyn DynTool> {
    Arc::new(UrlContext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{InvocationContext, InvocationOrigin, RunConfig, Session, SessionService};
    use crate::services::mem::InMemorySessionService;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    fn ctx() -> ToolContext {
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let inv = Arc::new(InvocationContext {
            app_name: "app".into(),
            user_id: "u".into(),
            invocation_id: "inv".into(),
            session: Arc::new(Mutex::new(Session::new("app", "u", "s"))),
            session_service: svc,
            artifact_service: None,
            memory_service: None,
            credential_service: None,
            run_config: RunConfig::default(),
            origin: InvocationOrigin::Api,
            user_content: None,
            llm_call_count: Arc::new(Mutex::new(0)),
            cancellation: Default::default(),
            attributes: Arc::new(Mutex::new(HashMap::new())),
        });
        ToolContext::new(inv)
    }

    #[tokio::test]
    async fn injects_url_context_tool_once() {
        let tool = url_context_tool();
        let mut req = LlmRequest::default();
        let mut tctx = ctx();
        tool.process_llm_request(&mut req, &mut tctx).await.unwrap();
        tool.process_llm_request(&mut req, &mut tctx).await.unwrap();
        assert_eq!(
            req.config
                .tools
                .iter()
                .filter(|t| matches!(t, Tool::UrlContext {}))
                .count(),
            1
        );
    }
}
