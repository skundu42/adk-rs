//! Built-in tools that mirror Python ADK's `transfer_to_agent_tool` and
//! `exit_loop_tool`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, ToolContext};
use crate::error::Result;
use crate::genai_types::{FunctionDeclaration, Schema};

/// Transfer-to-agent tool. Setting `transfer_to_agent` on the [`ToolContext`]
/// causes the runner to switch control to the named agent.
#[derive(Debug)]
struct TransferToAgent;

#[async_trait]
impl DynTool for TransferToAgent {
    fn name(&self) -> &str {
        "transfer_to_agent"
    }
    fn description(&self) -> &str {
        "Transfer control to another agent by name. Use this when the user's request is better handled by a different agent."
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(
            FunctionDeclaration::new(self.name(), self.description()).with_parameters(
                Schema::object()
                    .property(
                        "agent_name",
                        Schema::string().with_description("The name of the agent to transfer to."),
                    )
                    .require("agent_name"),
            ),
        )
    }
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        let name = args
            .get("agent_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::Error::invalid_input("agent_name must be a string"))?;
        ctx.transfer_to_agent = Some(name.to_string());
        Ok(serde_json::json!({"status": "ok"}))
    }
}

/// Construct the transfer-to-agent built-in.
#[must_use]
pub fn transfer_to_agent_tool() -> Arc<dyn DynTool> {
    Arc::new(TransferToAgent)
}

/// Exit-loop tool. Sets `escalate=true` to break a `LoopAgent`.
#[derive(Debug)]
struct ExitLoop;

#[async_trait]
impl DynTool for ExitLoop {
    fn name(&self) -> &str {
        "exit_loop"
    }
    fn description(&self) -> &str {
        "Exit the current LoopAgent iteration. Use this when the loop's objective has been met."
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(FunctionDeclaration::new(self.name(), self.description()))
    }
    async fn run(&self, _args: Value, ctx: &mut ToolContext) -> Result<Value> {
        ctx.escalate = true;
        Ok(serde_json::json!({"status": "ok"}))
    }
}

/// Construct the exit-loop built-in.
#[must_use]
pub fn exit_loop() -> Arc<dyn DynTool> {
    Arc::new(ExitLoop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{GetSessionConfig, ListSessionsResponse, SessionService};
    use crate::core::{InvocationContext, InvocationOrigin, RunConfig, Session, State};
    use parking_lot::Mutex;
    use serde_json::json;
    use std::collections::HashMap;

    #[derive(Debug)]
    struct NoopSession;
    #[async_trait]
    impl SessionService for NoopSession {
        async fn create_session(
            &self,
            app: &str,
            user: &str,
            _: Option<State>,
            id: Option<&str>,
        ) -> Result<Session> {
            Ok(Session::new(app, user, id.unwrap_or("s")))
        }
        async fn get_session(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: GetSessionConfig,
        ) -> Result<Option<Session>> {
            Ok(None)
        }
        async fn list_sessions(&self, _: &str, _: &str) -> Result<ListSessionsResponse> {
            Ok(ListSessionsResponse::default())
        }
        async fn delete_session(&self, _: &str, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
    }

    fn ctx() -> ToolContext {
        let inv = Arc::new(InvocationContext {
            app_name: "app".into(),
            user_id: "u".into(),
            invocation_id: "inv-1".into(),
            session: Arc::new(Mutex::new(Session::new("app", "u", "s"))),
            session_service: Arc::new(NoopSession),
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
    async fn transfer_to_agent_sets_target() {
        let tool = transfer_to_agent_tool();
        let mut c = ctx();
        tool.run(json!({"agent_name": "specialist"}), &mut c)
            .await
            .unwrap();
        assert_eq!(c.transfer_to_agent.as_deref(), Some("specialist"));
    }

    #[tokio::test]
    async fn transfer_to_agent_rejects_missing_arg() {
        let tool = transfer_to_agent_tool();
        let mut c = ctx();
        let err = tool.run(json!({}), &mut c).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("agent_name"));
        assert!(c.transfer_to_agent.is_none());
    }

    #[tokio::test]
    async fn exit_loop_sets_escalate() {
        let tool = exit_loop();
        let mut c = ctx();
        assert!(!c.escalate);
        tool.run(json!({}), &mut c).await.unwrap();
        assert!(c.escalate);
    }

    #[test]
    fn declarations_include_required_params() {
        let d = transfer_to_agent_tool().declaration().unwrap();
        assert_eq!(d.name, "transfer_to_agent");
        assert!(d.parameters.is_some());
        let d = exit_loop().declaration().unwrap();
        assert_eq!(d.name, "exit_loop");
    }
}
