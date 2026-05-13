//! [`AgentTool`] — wraps another [`BaseAgent`] as a [`DynTool`] so a parent
//! agent can invoke it like a function. Spawns a sub-`Runner` with the same
//! services as the caller and forwards the user's request as a turn.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;

use crate::agents::BaseAgent;
use crate::core::{DynTool, InvocationContext, InvocationOrigin, ToolContext};
use crate::error::{Error, Result};
use crate::genai_types::{Content, FunctionDeclaration, Schema};

/// Tool wrapper around a child [`BaseAgent`]. Calling the tool runs the child
/// against the supplied `request` and returns the concatenated text of all
/// events the child emitted.
pub struct AgentTool {
    agent: Arc<dyn BaseAgent>,
    description: String,
}

impl std::fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool")
            .field("agent", &self.agent.name())
            .finish_non_exhaustive()
    }
}

impl AgentTool {
    /// Wrap `agent` under the same name as the agent.
    #[must_use]
    pub fn wrap(agent: Arc<dyn BaseAgent>) -> Arc<Self> {
        let description = agent.description().to_string();
        Arc::new(Self { agent, description })
    }

    /// Wrap with a custom description (overrides the child's).
    #[must_use]
    pub fn wrap_with_description(
        agent: Arc<dyn BaseAgent>,
        description: impl Into<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent,
            description: description.into(),
        })
    }
}

#[async_trait]
impl DynTool for AgentTool {
    fn name(&self) -> &str {
        self.agent.name()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(
            FunctionDeclaration::new(self.name(), self.description()).with_parameters(
                Schema::object()
                    .property(
                        "request",
                        Schema::string().with_description("Task to delegate to the sub-agent."),
                    )
                    .require("request"),
            ),
        )
    }
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        let request = args
            .get("request")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid_input("request must be a string"))?;

        // Construct a sub-invocation that shares services + session with the
        // parent. The sub-agent's events are accumulated locally and returned
        // as a single response.
        let sub_ctx = Arc::new(InvocationContext {
            app_name: ctx.invocation.app_name.clone(),
            user_id: ctx.invocation.user_id.clone(),
            invocation_id: format!("{}.sub.{}", ctx.invocation.invocation_id, self.name()),
            session: ctx.invocation.session.clone(),
            session_service: ctx.invocation.session_service.clone(),
            artifact_service: ctx.invocation.artifact_service.clone(),
            memory_service: ctx.invocation.memory_service.clone(),
            credential_service: ctx.invocation.credential_service.clone(),
            run_config: ctx.invocation.run_config.clone(),
            origin: InvocationOrigin::Api,
            user_content: Some(Content::user_text(request)),
            llm_call_count: ctx.invocation.llm_call_count.clone(),
            attributes: ctx.invocation.attributes.clone(),
        });
        let mut stream = self.agent.clone().run(sub_ctx).await?;
        let mut out = String::new();
        let mut last_error: Option<String> = None;
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if let Some(c) = &ev.response.content {
                let t = c.text_concat();
                if !t.is_empty() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&t);
                }
            }
            if let Some(err) = &ev.response.error_message {
                last_error = Some(err.clone());
            }
        }
        if let Some(e) = last_error {
            Ok(serde_json::json!({"text": out, "error": e}))
        } else {
            Ok(serde_json::json!({"text": out}))
        }
    }
}
