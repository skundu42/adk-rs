//! `get_user_choice` — long-running HITL tool. The model calls it with a
//! list of options; the agent stops and returns control to the caller, which
//! is expected to display the prompt and resubmit the chosen option as a
//! follow-up `FunctionResponse`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, ToolContext};
use crate::error::{Error, Result};
use crate::genai_types::{FunctionDeclaration, Schema};

/// Pause the agent and surface a multiple-choice prompt to the user.
#[derive(Debug)]
struct GetUserChoice;

#[async_trait]
impl DynTool for GetUserChoice {
    fn name(&self) -> &str {
        "get_user_choice"
    }
    fn description(&self) -> &str {
        "Ask the user to pick one of several options before continuing. \
         Use this for human-in-the-loop confirmations."
    }
    fn is_long_running(&self) -> bool {
        true
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(
            FunctionDeclaration::new(self.name(), self.description()).with_parameters(
                Schema::object()
                    .property(
                        "prompt",
                        Schema::string().with_description("Question for the user."),
                    )
                    .property(
                        "options",
                        Schema::array(Schema::string()).with_description("Available answers."),
                    )
                    .require("prompt")
                    .require("options"),
            ),
        )
    }
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        let prompt = args
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid_input("prompt must be a string"))?;
        let options: Vec<String> = args
            .get("options")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| Error::invalid_input("options must be an array of strings"))?;
        ctx.long_running = true;
        Ok(serde_json::json!({
            "status": "awaiting_user",
            "prompt": prompt,
            "options": options,
        }))
    }
}

/// Construct the `get_user_choice` tool.
#[must_use]
pub fn get_user_choice_tool() -> Arc<dyn DynTool> {
    Arc::new(GetUserChoice)
}
