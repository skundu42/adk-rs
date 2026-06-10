//! Gemini server-side built-in tools: `google_search` and
//! `built_in_code_execution`.
//!
//! These are passive [`DynTool`] handles — they expose no
//! [`FunctionDeclaration`] to the model and never receive a `FunctionCall`.
//! Their only job is to inject the matching wire-level [`Tool`] variant into
//! `req.config.tools` so the Gemini API runs the corresponding server-side
//! capability (web grounding via Google Search, or sandboxed Python
//! execution).
//!
//! The same handles are accepted by other providers as no-ops: they only
//! mutate `req.config.tools`, which non-Gemini providers either ignore or
//! reject server-side.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, LlmRequest, ToolContext};
use crate::error::Result;
use crate::genai_types::{FunctionDeclaration, Tool};

#[derive(Debug, Default)]
struct GoogleSearch;

#[async_trait]
impl DynTool for GoogleSearch {
    fn name(&self) -> &str {
        "google_search"
    }
    fn description(&self) -> &str {
        "Grounds the model's response in fresh Google Search results \
         (Gemini server-side tool)."
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
            .any(|t| matches!(t, Tool::GoogleSearch {}))
        {
            req.config.tools.push(Tool::GoogleSearch {});
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct BuiltInCodeExecution;

#[async_trait]
impl DynTool for BuiltInCodeExecution {
    fn name(&self) -> &str {
        "built_in_code_execution"
    }
    fn description(&self) -> &str {
        "Lets the model run Python in a Gemini-hosted sandbox to compute \
         intermediate results (Gemini server-side tool)."
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
            .any(|t| matches!(t, Tool::CodeExecution {}))
        {
            req.config.tools.push(Tool::CodeExecution {});
        }
        Ok(())
    }
}

/// Opt-in to Gemini's built-in Google Search grounding. The model decides
/// when to issue searches; grounding citations are returned via
/// [`crate::core::LlmResponse::grounding_metadata`].
#[must_use]
pub fn google_search_tool() -> Arc<dyn DynTool> {
    Arc::new(GoogleSearch)
}

/// Opt-in to Gemini's server-side code execution. The model emits
/// [`crate::genai_types::Part::ExecutableCode`] parts which Gemini executes
/// itself and returns as [`crate::genai_types::Part::CodeExecutionResult`].
/// No local executor is required.
#[must_use]
pub fn built_in_code_execution_tool() -> Arc<dyn DynTool> {
    Arc::new(BuiltInCodeExecution)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{InvocationContext, InvocationOrigin, RunConfig, Session, SessionService};
    use crate::services::mem::InMemorySessionService;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    fn tctx() -> ToolContext {
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
            root_agent: None,
        });
        ToolContext::new(inv)
    }

    #[tokio::test]
    async fn google_search_injects_wire_tool() {
        let tool = google_search_tool();
        let mut req = LlmRequest::default();
        let mut ctx = tctx();
        tool.process_llm_request(&mut req, &mut ctx).await.unwrap();
        assert!(
            req.config
                .tools
                .iter()
                .any(|t| matches!(t, Tool::GoogleSearch {}))
        );
    }

    #[tokio::test]
    async fn code_execution_injects_wire_tool() {
        let tool = built_in_code_execution_tool();
        let mut req = LlmRequest::default();
        let mut ctx = tctx();
        tool.process_llm_request(&mut req, &mut ctx).await.unwrap();
        assert!(
            req.config
                .tools
                .iter()
                .any(|t| matches!(t, Tool::CodeExecution {}))
        );
    }

    #[tokio::test]
    async fn declarations_are_hidden_from_model() {
        assert!(google_search_tool().declaration().is_none());
        assert!(built_in_code_execution_tool().declaration().is_none());
    }
}
