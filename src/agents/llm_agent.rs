//! [`LlmAgent`] — an LLM-powered agent that may use tools and sub-agents.

use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::StreamExt;
use futures::future::BoxFuture;
use tracing::{debug, instrument};

use crate::core::{
    DynTool, Event, EventActions, EventStream, InvocationContext, LlmRequest, LlmResponse, Model,
    ReadonlyContext, ToolContext,
};
use crate::error::{Error, Result};
use crate::genai_types::{Content, FunctionResponse, Part, Role};

use crate::agents::base::BaseAgent;

/// Default model used when no model is provided.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// An async function that produces the system instruction for the agent.
pub type InstructionProvider =
    Arc<dyn for<'a> Fn(&'a ReadonlyContext) -> BoxFuture<'a, Result<String>> + Send + Sync>;

#[derive(Clone)]
enum Instruction {
    Static(String),
    Dynamic(InstructionProvider),
}

impl std::fmt::Debug for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(s) => f.debug_tuple("Static").field(s).finish(),
            Self::Dynamic(_) => f.debug_tuple("Dynamic").field(&"<fn>").finish(),
        }
    }
}

/// LLM-powered agent.
#[derive(Debug)]
pub struct LlmAgent {
    name: String,
    description: String,
    model: Arc<dyn Model>,
    instruction: Option<Instruction>,
    global_instruction: Option<Instruction>,
    tools: Vec<Arc<dyn DynTool>>,
    sub_agents: Vec<Arc<dyn BaseAgent>>,
    /// If true, disallow agent transfer (mirrors Python's `disallow_transfer_to_*`
    /// in a coarser form).
    disable_transfer: bool,
    /// Max iterations of the LLM↔tool loop within a single agent run.
    max_iterations: u32,
    /// Optional executor for `ExecutableCode` parts emitted by the model.
    #[cfg(feature = "code-exec")]
    code_executor: Option<Arc<dyn crate::code_exec::CodeExecutor>>,
}

impl LlmAgent {
    /// Start building.
    pub fn builder(name: impl Into<String>) -> LlmAgentBuilder {
        LlmAgentBuilder::new(name.into())
    }

    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Tools registered on this agent (direct only).
    pub fn tools(&self) -> &[Arc<dyn DynTool>] {
        &self.tools
    }

    /// Active model.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }
}

#[async_trait]
impl BaseAgent for LlmAgent {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn sub_agents(&self) -> &[Arc<dyn BaseAgent>] {
        &self.sub_agents
    }

    #[instrument(skip_all, fields(agent = %self.name, invocation = %ctx.invocation_id))]
    async fn run(self: Arc<Self>, ctx: Arc<InvocationContext>) -> Result<EventStream<'static>> {
        let me = self.clone();
        let ctx2 = ctx.clone();
        let stream = try_stream! {
            let mut req = build_request(&me, &ctx2).await?;
            let history: Vec<Content> = ctx2
                .session
                .lock()
                .events
                .iter()
                .filter_map(|e| e.response.content.clone())
                .collect();
            req.contents = history;
            if let Some(user) = &ctx2.user_content {
                req.contents.push(user.clone());
            }

            for _iter in 0..me.max_iterations {
                ctx2.check_and_inc_llm_call()?;
                debug!("LLM call iteration {}", _iter);
                let resp = me.model.generate_content(req.clone()).await?;
                let event = response_to_event(&me.name, &ctx2.invocation_id, resp.clone());

                // Persist on session.
                {
                    let mut sess = ctx2.session.lock();
                    sess.events.push(event.clone());
                }

                let calls = event.function_calls();
                if calls.is_empty() {
                    // No function calls. Before treating this as the final
                    // response, check whether the model emitted code that the
                    // agent should run.
                    #[cfg(feature = "code-exec")]
                    if let Some(executor) = me.code_executor.as_ref() {
                        let code_parts = extract_executable_code(&event);
                        if !code_parts.is_empty() {
                            yield event.clone();
                            let mut result_parts: Vec<Part> = Vec::new();
                            for (lang, code) in &code_parts {
                                let result = executor
                                    .execute_code(
                                        &ctx2,
                                        crate::code_exec::CodeExecutionInput {
                                            code: code.clone(),
                                            language: lang.clone(),
                                            ..Default::default()
                                        },
                                    )
                                    .await?;
                                let outcome = if result.stderr.is_empty() {
                                    crate::genai_types::part::Outcome::OutcomeOk
                                } else {
                                    crate::genai_types::part::Outcome::OutcomeFailed
                                };
                                result_parts.push(Part::CodeExecutionResult(
                                    crate::genai_types::part::CodeExecutionResult {
                                        outcome,
                                        output: Some(result.combined_output()),
                                    },
                                ));
                            }
                            let code_result_event = Event::new(
                                me.name.clone(),
                                LlmResponse {
                                    content: Some(Content { role: Role::Tool, parts: result_parts.clone() }),
                                    ..Default::default()
                                },
                            );
                            {
                                let mut sess = ctx2.session.lock();
                                sess.events.push(code_result_event.clone());
                            }
                            yield code_result_event;
                            // Append code + result into the next turn's contents.
                            if let Some(c) = event.response.content {
                                req.contents.push(c);
                            }
                            req.contents.push(Content {
                                role: Role::Tool,
                                parts: result_parts,
                            });
                            continue;
                        }
                    }
                    // Final response.
                    yield event;
                    return;
                }

                // Yield the assistant turn carrying the calls (clone so we
                // can also re-use the content below for history).
                let assistant_content = event.response.content.clone();
                yield event;

                // Resolve each call by dispatching the tool.
                let mut tool_responses = Vec::with_capacity(calls.len());
                let mut transfer: Option<String> = None;
                let mut escalate = false;
                let mut long_running_any = false;
                for fc in &calls {
                    let tool = req
                        .tools_dict
                        .get(&fc.name)
                        .cloned()
                        .ok_or_else(|| {
                            Error::from(crate::error::ToolError::Unknown(fc.name.clone()))
                        })?;
                    let mut tctx = ToolContext::new(ctx2.clone());
                    if let Some(id) = &fc.id {
                        tctx.function_call_id = Some(id.clone());
                    }

                    // Resolve auth before dispatch, if the tool declared a config.
                    let (auth_pending, value) = resolve_auth_and_run(
                        tool.as_ref(),
                        fc.args.clone(),
                        &mut tctx,
                    )
                    .await?;

                    if let Some(t) = tctx.transfer_to_agent.take() {
                        if !me.disable_transfer { transfer = Some(t); }
                    }
                    if tctx.escalate { escalate = true; }
                    let will_continue = if tool.is_long_running() || tctx.long_running {
                        long_running_any = true;
                        Some(true)
                    } else if auth_pending {
                        // Bubble the pending auth response back via will_continue
                        // so the caller knows to resume after consent.
                        long_running_any = true;
                        Some(true)
                    } else {
                        None
                    };
                    tool_responses.push(
                        FunctionResponse { id: fc.id.clone(), name: fc.name.clone(), response: value, will_continue, scheduling: None }
                    );
                }

                // Emit a tool-response event.
                let tool_event = function_response_event(&me.name, &ctx2.invocation_id, tool_responses.clone());
                {
                    let mut sess = ctx2.session.lock();
                    sess.events.push(tool_event.clone());
                }
                yield tool_event;

                // Apply transfer / escalate before next turn.
                if let Some(target) = transfer {
                    let sub = me
                        .find_agent(&target)
                        .ok_or_else(|| Error::not_found(format!("agent {target}")))?;
                    let mut sub_stream = Box::pin(sub.run(ctx2.clone()).await?);
                    while let Some(ev) = sub_stream.next().await {
                        yield ev?;
                    }
                    return;
                }
                if escalate {
                    // Escalation propagates outward; we emit a marker event and stop.
                    let mut esc = Event::new(me.name.clone(), LlmResponse::default());
                    esc.invocation_id = ctx2.invocation_id.clone();
                    esc.actions.escalate = Some(true);
                    yield esc;
                    return;
                }
                if long_running_any {
                    // Either a long-running tool returned a handle, or a tool
                    // needs interactive consent. Stop the loop and let the
                    // caller resume on a follow-up invocation.
                    return;
                }

                // Update conversation history with assistant call + tool resp.
                if let Some(c) = assistant_content { req.contents.push(c); }
                req.contents.push(Content {
                    role: Role::Tool,
                    parts: tool_responses
                        .into_iter()
                        .map(Part::FunctionResponse)
                        .collect(),
                });
            }

            // Iteration budget exhausted; emit a fail-safe event.
            let mut e = Event::new(me.name.clone(), LlmResponse {
                error_code: Some("MAX_ITERATIONS".into()),
                error_message: Some("agent exhausted its iteration budget".into()),
                ..Default::default()
            });
            e.invocation_id = ctx2.invocation_id.clone();
            yield e;
        };
        Ok(Box::pin(stream))
    }
}

async fn build_request(agent: &LlmAgent, ctx: &Arc<InvocationContext>) -> Result<LlmRequest> {
    let mut req = LlmRequest {
        model: Some(agent.model.name().to_string()),
        ..Default::default()
    };

    // Instructions.
    let ro = ReadonlyContext::new(ctx.clone());
    if let Some(inst) = &agent.global_instruction {
        let s = resolve_instruction(inst, &ro).await?;
        req.append_system_text(&s);
    }
    if let Some(inst) = &agent.instruction {
        let s = resolve_instruction(inst, &ro).await?;
        req.append_system_text(&s);
    }

    // Tools.
    let mut tctx = ToolContext::new(ctx.clone());
    for t in &agent.tools {
        t.process_llm_request(&mut req, &mut tctx).await?;
        req.tools_dict.insert(t.name().to_string(), t.clone());
    }
    Ok(req)
}

async fn resolve_instruction(i: &Instruction, ctx: &ReadonlyContext) -> Result<String> {
    match i {
        Instruction::Static(s) => Ok(s.clone()),
        Instruction::Dynamic(f) => f(ctx).await,
    }
}

fn response_to_event(author: &str, invocation_id: &str, resp: LlmResponse) -> Event {
    Event {
        id: Event::new_id(),
        invocation_id: invocation_id.to_string(),
        author: author.to_string(),
        timestamp: crate::core::session::now_secs(),
        branch: None,
        response: resp,
        actions: EventActions::default(),
        long_running_tool_ids: None,
        partial: None,
        turn_complete: Some(true),
    }
}

fn function_response_event(
    author: &str,
    invocation_id: &str,
    responses: Vec<FunctionResponse>,
) -> Event {
    let content = Content {
        role: Role::Tool,
        parts: responses.into_iter().map(Part::FunctionResponse).collect(),
    };
    Event {
        id: Event::new_id(),
        invocation_id: invocation_id.to_string(),
        author: author.to_string(),
        timestamp: crate::core::session::now_secs(),
        branch: None,
        response: LlmResponse {
            content: Some(content),
            ..LlmResponse::default()
        },
        actions: EventActions::default(),
        long_running_tool_ids: None,
        partial: None,
        turn_complete: None,
    }
}

/// Extract `(language, code)` from any `ExecutableCode` parts in the event.
#[cfg(feature = "code-exec")]
fn extract_executable_code(event: &Event) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(c) = event.response.content.as_ref() {
        for p in &c.parts {
            if let Part::ExecutableCode(ec) = p {
                let lang = ec.language.to_lowercase();
                out.push((lang, ec.code.clone()));
            }
        }
    }
    out
}

/// Resolve auth (if the tool declared a config), inject the credential into
/// `tctx`, and dispatch the tool. Returns `(auth_pending, response_value)`.
///
/// When auth is enabled (`feature = "auth"`):
/// - If the credential resolves cleanly → injects it and calls `tool.run`.
/// - If the tool needs interactive consent → returns
///   `(true, AuthConfig-as-JSON)` *without* calling `tool.run`. The agent
///   surfaces this as a `FunctionResponse` with the synthetic
///   `adk_request_credential` semantics so the caller can drive the OAuth2
///   redirect and resubmit.
/// - If the tool's config is misconfigured → returns an error JSON.
///
/// When `feature = "auth"` is off, `auth_config()` is `None` for every
/// tool so this just dispatches.
async fn resolve_auth_and_run(
    tool: &dyn DynTool,
    args: serde_json::Value,
    tctx: &mut ToolContext,
) -> Result<(bool, serde_json::Value)> {
    #[cfg(feature = "auth")]
    {
        if let Some(cfg) = tool.auth_config() {
            let mgr = crate::auth::CredentialManager::new(cfg.clone());
            let credentials = tctx.invocation.credential_service.clone();
            let outcome = mgr
                .resolve(
                    &tctx.invocation.app_name,
                    &tctx.invocation.user_id,
                    credentials.as_deref(),
                )
                .await?;
            match outcome {
                crate::auth::ResolveOutcome::Ready(cred) => {
                    tctx.auth_credential = Some(cred);
                }
                crate::auth::ResolveOutcome::NeedsUserConsent(pending) => {
                    // Defer: don't call the tool. The caller resubmits a
                    // FunctionResponse(name="adk_request_credential", ...)
                    // with the exchanged credential filled in; the next
                    // invocation absorbs it via AuthPreprocessor.
                    let value = serde_json::to_value(&pending).unwrap_or(serde_json::Value::Null);
                    return Ok((true, value));
                }
                crate::auth::ResolveOutcome::Misconfigured(msg) => {
                    return Ok((false, serde_json::json!({"error": msg})));
                }
            }
        }
    }
    #[cfg(not(feature = "auth"))]
    let _ = tool.auth_config(); // suppress unused-trait-method warning

    let value = match tool.run(args, tctx).await {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error": e.to_string()}),
    };
    Ok((false, value))
}

/// Builder for [`LlmAgent`].
#[derive(Default)]
pub struct LlmAgentBuilder {
    name: String,
    description: String,
    model: Option<Arc<dyn Model>>,
    instruction: Option<Instruction>,
    global_instruction: Option<Instruction>,
    tools: Vec<Arc<dyn DynTool>>,
    sub_agents: Vec<Arc<dyn BaseAgent>>,
    disable_transfer: bool,
    max_iterations: Option<u32>,
    #[cfg(feature = "code-exec")]
    code_executor: Option<Arc<dyn crate::code_exec::CodeExecutor>>,
}

impl LlmAgentBuilder {
    /// Construct.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Description.
    #[must_use]
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    /// Provider model.
    #[must_use]
    pub fn model(mut self, m: Arc<dyn Model>) -> Self {
        self.model = Some(m);
        self
    }

    /// Static system instruction.
    #[must_use]
    pub fn instruction(mut self, s: impl Into<String>) -> Self {
        self.instruction = Some(Instruction::Static(s.into()));
        self
    }

    /// Dynamic instruction (async).
    #[must_use]
    pub fn instruction_dyn(mut self, p: InstructionProvider) -> Self {
        self.instruction = Some(Instruction::Dynamic(p));
        self
    }

    /// Global instruction (prefixed before `instruction`).
    #[must_use]
    pub fn global_instruction(mut self, s: impl Into<String>) -> Self {
        self.global_instruction = Some(Instruction::Static(s.into()));
        self
    }

    /// Register a tool.
    #[must_use]
    pub fn tool(mut self, t: Arc<dyn DynTool>) -> Self {
        self.tools.push(t);
        self
    }

    /// Register multiple tools.
    #[must_use]
    pub fn tools(mut self, ts: impl IntoIterator<Item = Arc<dyn DynTool>>) -> Self {
        self.tools.extend(ts);
        self
    }

    /// Register a sub-agent.
    #[must_use]
    pub fn sub_agent(mut self, a: Arc<dyn BaseAgent>) -> Self {
        self.sub_agents.push(a);
        self
    }

    /// Disable transfer-to-agent tool emission.
    #[must_use]
    pub fn disable_transfer(mut self, yes: bool) -> Self {
        self.disable_transfer = yes;
        self
    }

    /// Cap iterations of the LLM↔tool loop (default: 16).
    #[must_use]
    pub fn max_iterations(mut self, n: u32) -> Self {
        self.max_iterations = Some(n);
        self
    }

    /// Attach a [`crate::code_exec::CodeExecutor`]. When set, the agent will
    /// extract `ExecutableCode` parts from each LLM response and dispatch
    /// them to the executor, feeding back `CodeExecutionResult` parts.
    #[cfg(feature = "code-exec")]
    #[must_use]
    pub fn code_executor(mut self, ex: Arc<dyn crate::code_exec::CodeExecutor>) -> Self {
        self.code_executor = Some(ex);
        self
    }

    /// Build.
    pub fn build(self) -> Result<LlmAgent> {
        let model = self
            .model
            .ok_or_else(|| Error::config("LlmAgent requires a `model`"))?;
        if self.name.is_empty() {
            return Err(Error::config("LlmAgent requires a non-empty `name`"));
        }
        Ok(LlmAgent {
            name: self.name,
            description: self.description,
            model,
            instruction: self.instruction,
            global_instruction: self.global_instruction,
            tools: self.tools,
            sub_agents: self.sub_agents,
            disable_transfer: self.disable_transfer,
            max_iterations: self.max_iterations.unwrap_or(16),
            #[cfg(feature = "code-exec")]
            code_executor: self.code_executor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::core::testing::MockModel;
    use crate::core::{InvocationContext, InvocationOrigin, RunConfig, Session};
    use crate::services::mem::InMemorySessionService;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    fn build_ctx(
        svc: Arc<dyn crate::core::SessionService>,
        user_text: &str,
    ) -> Arc<InvocationContext> {
        Arc::new(InvocationContext {
            app_name: "app".into(),
            user_id: "u".into(),
            invocation_id: InvocationContext::new_id(),
            session: Arc::new(Mutex::new(Session::new("app", "u", "s"))),
            session_service: svc,
            artifact_service: None,
            memory_service: None,
            credential_service: None,
            run_config: RunConfig::default(),
            origin: InvocationOrigin::Api,
            user_content: Some(Content::user_text(user_text)),
            llm_call_count: Arc::new(Mutex::new(0)),
            attributes: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[tokio::test]
    async fn llm_agent_runs_single_turn() {
        let model = Arc::new(MockModel::new("mock-1"));
        model.push_text("hello there");
        let agent = Arc::new(
            LlmAgent::builder("greeter")
                .description("greets")
                .model(model.clone() as Arc<dyn Model>)
                .instruction("Be friendly.")
                .build()
                .unwrap(),
        );
        let svc: Arc<dyn crate::core::SessionService> = Arc::new(InMemorySessionService::new());
        let ctx = build_ctx(svc, "hi");
        let mut stream = agent.run(ctx).await.unwrap();
        let mut events = Vec::new();
        while let Some(e) = stream.next().await {
            events.push(e.unwrap());
        }
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].response.content.as_ref().unwrap().text_concat(),
            "hello there"
        );
    }
}
