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
                if req.contents.last() != Some(user) {
                    req.contents.push(user.clone());
                }
            }

            let replayed_responses = replay_resumed_tool_calls(&ctx2, &req).await?;
            if !replayed_responses.is_empty() {
                let replay_event = function_response_event(
                    &me.name,
                    &ctx2.invocation_id,
                    replayed_responses.clone(),
                );
                {
                    let mut sess = ctx2.session.lock();
                    sess.events.push(replay_event.clone());
                }
                yield replay_event;
                req.contents.push(Content {
                    role: Role::Tool,
                    parts: replayed_responses
                        .into_iter()
                        .map(Part::FunctionResponse)
                        .collect(),
                });
            }

            for _iter in 0..me.max_iterations {
                if ctx2.is_cancelled() {
                    let mut e = cancellation_event(&me.name, &ctx2.invocation_id);
                    {
                        let mut sess = ctx2.session.lock();
                        sess.events.push(e.clone());
                    }
                    e.invocation_id = ctx2.invocation_id.clone();
                    yield e;
                    return;
                }
                ctx2.check_and_inc_llm_call()?;
                debug!("LLM call iteration {}", _iter);
                let resp = me.model.generate_content(req.clone()).await?;
                let mut event = response_to_event(&me.name, &ctx2.invocation_id, resp.clone());

                // Gemini may omit `FunctionCall.id`. Synthesize a stable id
                // for any id-less call BEFORE pushing the event into the
                // session so every downstream consumer (session.events,
                // ToolContext, FunctionResponse, replay matcher, auth
                // preprocessor) sees the same value. Without this, auth-pending
                // tool calls cannot be resumed after consent.
                ensure_function_call_ids(&mut event);

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
                            let max_attempts = executor.error_retry_attempts().max(1);
                            for (lang, code) in &code_parts {
                                let mut last_err: Option<crate::error::Error> = None;
                                let mut delivered = false;
                                for _attempt in 0..max_attempts {
                                    match executor
                                        .execute_code(
                                            &ctx2,
                                            crate::code_exec::CodeExecutionInput {
                                                code: code.clone(),
                                                language: lang.clone(),
                                                ..Default::default()
                                            },
                                        )
                                        .await
                                    {
                                        Ok(result) => {
                                            // Outcome is driven by the child's
                                            // exit code, not by stderr presence
                                            // (stderr is routine for warnings).
                                            let outcome = if result.is_success() {
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
                                            delivered = true;
                                            break;
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "code executor error (will retry): {e}"
                                            );
                                            last_err = Some(e);
                                        }
                                    }
                                }
                                if !delivered {
                                    // Out of retries — surface as a failed
                                    // CodeExecutionResult rather than aborting
                                    // the whole agent run.
                                    let msg = last_err
                                        .map(|e| e.to_string())
                                        .unwrap_or_else(|| "code executor failed".into());
                                    result_parts.push(Part::CodeExecutionResult(
                                        crate::genai_types::part::CodeExecutionResult {
                                            outcome:
                                                crate::genai_types::part::Outcome::OutcomeFailed,
                                            output: Some(msg),
                                        },
                                    ));
                                }
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
                let mut long_running_tool_ids = Vec::new();
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
                        long_running_tool_ids.push(
                            fc.id.clone().unwrap_or_else(|| fc.name.clone())
                        );
                        Some(true)
                    } else if auth_pending {
                        // Bubble the pending auth response back via will_continue
                        // so the caller knows to resume after consent.
                        long_running_any = true;
                        long_running_tool_ids.push(
                            fc.id.clone().unwrap_or_else(|| fc.name.clone())
                        );
                        Some(true)
                    } else {
                        None
                    };
                    let response_name = if auth_pending {
                        crate::auth::REQUEST_CREDENTIAL_FUNCTION_NAME.to_string()
                    } else {
                        fc.name.clone()
                    };
                    tool_responses.push(
                        FunctionResponse { id: fc.id.clone(), name: response_name, response: value, will_continue, scheduling: None }
                    );
                }

                // Emit a tool-response event.
                let mut tool_event = function_response_event(&me.name, &ctx2.invocation_id, tool_responses.clone());
                if !long_running_tool_ids.is_empty() {
                    tool_event.long_running_tool_ids = Some(long_running_tool_ids);
                }
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

/// Walk `event.response.content.parts` and assign a synthesized id to any
/// `FunctionCall` part that lacks one (Gemini may omit `id`). The mutation
/// must run **before** the event is persisted into `session.events` so every
/// downstream consumer (replay matcher, auth preprocessor, FunctionResponse
/// id, ToolContext.function_call_id) observes the same value.
fn ensure_function_call_ids(event: &mut Event) {
    let Some(content) = event.response.content.as_mut() else {
        return;
    };
    for part in &mut content.parts {
        if let Part::FunctionCall(fc) = part {
            if fc.id.is_none() {
                fc.id = Some(format!("adk-fc-{}", uuid::Uuid::new_v4()));
            }
        }
    }
}

/// Build a terminal event signalling that the agent observed a
/// cancellation and stopped before the next LLM call. The event carries a
/// `CANCELLED` error code (mirroring Python ADK's
/// `Event.error_code = "CANCELLED"`) so downstream consumers can
/// distinguish a cancel from an organic stop or a budget exhaustion.
fn cancellation_event(author: &str, invocation_id: &str) -> Event {
    let mut e = Event::new(
        author,
        LlmResponse {
            error_code: Some("CANCELLED".into()),
            error_message: Some("invocation was cancelled".into()),
            ..LlmResponse::default()
        },
    );
    e.invocation_id = invocation_id.to_string();
    e
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

async fn replay_resumed_tool_calls(
    ctx: &Arc<InvocationContext>,
    req: &LlmRequest,
) -> Result<Vec<FunctionResponse>> {
    let ids = resumed_tool_call_ids(ctx);
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let events = ctx.session.lock().events.clone();
    let mut responses = Vec::new();
    for id in ids {
        let Some(fc) = events
            .iter()
            .flat_map(Event::function_calls)
            .find(|fc| fc.id.as_deref() == Some(id.as_str()))
        else {
            continue;
        };
        let tool = req
            .tools_dict
            .get(&fc.name)
            .cloned()
            .ok_or_else(|| Error::from(crate::error::ToolError::Unknown(fc.name.clone())))?;
        let mut tctx = ToolContext::new(ctx.clone());
        tctx.function_call_id = fc.id.clone();
        let (auth_pending, value) =
            resolve_auth_and_run(tool.as_ref(), fc.args.clone(), &mut tctx).await?;
        let name = if auth_pending {
            crate::auth::REQUEST_CREDENTIAL_FUNCTION_NAME.to_string()
        } else {
            fc.name.clone()
        };
        responses.push(FunctionResponse {
            id: fc.id.clone(),
            name,
            response: value,
            will_continue: auth_pending.then_some(true),
            scheduling: None,
        });
    }
    Ok(responses)
}

fn resumed_tool_call_ids(ctx: &InvocationContext) -> Vec<String> {
    ctx.attributes
        .lock()
        .get("auth.resumed_tool_call_ids")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
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
            cancellation: Default::default(),
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

    /// Regression for P1#1: a model response carrying a `FunctionCall` with
    /// `id == None` (Gemini's default) must get a synthesised stable id
    /// before being persisted into the session. Without it, auth-pending
    /// tool calls can never be resumed after consent.
    #[test]
    fn ensure_function_call_ids_synthesises_ids() {
        use crate::genai_types::{Content, FunctionCall, Part, Role};

        let mut event = Event::new(
            "agent",
            LlmResponse {
                content: Some(Content {
                    role: Role::Model,
                    parts: vec![
                        Part::FunctionCall(FunctionCall::new(
                            "without_id",
                            serde_json::json!({"x": 1}),
                        )),
                        Part::FunctionCall(
                            FunctionCall::new("with_id", serde_json::json!({}))
                                .with_id("pre-existing"),
                        ),
                    ],
                }),
                ..Default::default()
            },
        );
        ensure_function_call_ids(&mut event);

        let calls = event.function_calls();
        assert_eq!(calls.len(), 2);

        // First call: missing id should be filled with a stable synthesised value.
        let first = calls.iter().find(|fc| fc.name == "without_id").unwrap();
        let id = first.id.as_deref().expect("synthesised id");
        assert!(
            id.starts_with("adk-fc-"),
            "synthesised id should be prefixed for traceability, got {id:?}"
        );
        // Same event re-serialised has the same id (mutation is in-place).
        assert_eq!(
            event
                .response
                .content
                .as_ref()
                .unwrap()
                .parts
                .iter()
                .find_map(|p| match p {
                    Part::FunctionCall(fc) => fc.id.clone(),
                    _ => None,
                }),
            Some(id.to_string())
        );

        // Second call: pre-existing id is preserved.
        let second = calls.iter().find(|fc| fc.name == "with_id").unwrap();
        assert_eq!(second.id.as_deref(), Some("pre-existing"));
    }
}
