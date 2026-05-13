//! Built-in tools that mirror Python ADK's `transfer_to_agent_tool` and
//! `exit_loop_tool`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use adk_rs_core::{DynTool, ToolContext};
use adk_rs_error::Result;
use adk_rs_genai_types::{FunctionDeclaration, Schema};

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
            .ok_or_else(|| adk_rs_error::Error::invalid_input("agent_name must be a string"))?;
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
