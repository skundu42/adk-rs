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
use crate::genai_types::{Content, FunctionResponse, Part, Role, Schema};

use crate::agents::base::BaseAgent;

/// Default model used when no model is provided.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// How much conversation history the agent sees (mirrors Python
/// `LlmAgent.include_contents`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IncludeContents {
    /// Full session history (default).
    #[default]
    Default,
    /// No history: only the system instruction and the current turn's user
    /// content are sent. Useful for stateless steps in workflow pipelines.
    None,
}

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
    /// Cache-stable instruction prefix. Never templated or re-evaluated, so
    /// the system instruction stays byte-identical across turns — the
    /// prerequisite for provider-side context caching. When set, the dynamic
    /// `instruction` is appended to the request *contents* instead of the
    /// system instruction (mirrors Python ADK).
    static_instruction: Option<Content>,
    tools: Vec<Arc<dyn DynTool>>,
    sub_agents: Vec<Arc<dyn BaseAgent>>,
    /// If true, disallow agent transfer (mirrors Python's `disallow_transfer_to_*`
    /// in a coarser form).
    disable_transfer: bool,
    /// Max iterations of the LLM↔tool loop within a single agent run.
    max_iterations: u32,
    /// State key the final response is saved to (via `state_delta`).
    output_key: Option<String>,
    /// Structured-output schema; forces JSON responses and, with
    /// `output_key`, stores the parsed JSON in state.
    output_schema: Option<Schema>,
    /// How much conversation history the model sees.
    include_contents: IncludeContents,
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
            let (mut req, deferred_instructions) = build_request(&me, &ctx2).await?;
            let history: Vec<Content> = match me.include_contents {
                IncludeContents::None => Vec::new(),
                IncludeContents::Default => {
                    crate::core::history_with_compaction(&ctx2.session.lock().events)
                }
            };
            req.contents = history;
            if let Some(user) = &ctx2.user_content {
                if req.contents.last() != Some(user) {
                    req.contents.push(user.clone());
                }
            }
            // With a static_instruction present, dynamic instructions ride in
            // the contents (after the user turn) so the cached system prefix
            // stays stable.
            if let Some(text) = deferred_instructions {
                req.contents.push(Content::user_text(text));
            }

            let replayed_responses = replay_resumed_tool_calls(&ctx2, &req, &me.name).await?;
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
                    if let Some(key) = &me.output_key {
                        if let Some(v) = output_value(&event, me.output_schema.is_some()) {
                            event.actions.state_delta.insert(key.clone(), v);
                            // The session already holds a pre-stamp copy of
                            // this event (pushed above); replace it so the
                            // in-memory view matches what the runner persists.
                            let mut sess = ctx2.session.lock();
                            if let Some(pos) =
                                sess.events.iter().rposition(|e| e.id == event.id)
                            {
                                sess.events[pos] = event.clone();
                            }
                        }
                    }
                    yield event;
                    return;
                }

                // Yield the assistant turn carrying the calls (clone so we
                // can also re-use the content below for history).
                let assistant_content = event.response.content.clone();
                yield event;

                // Resolve each call by dispatching the tool through the
                // gate pipeline (confirmation → auth → run).
                let mut tool_responses = Vec::with_capacity(calls.len());
                let mut transfer: Option<String> = None;
                let mut escalate = false;
                let mut long_running_any = false;
                let mut long_running_tool_ids = Vec::new();
                let mut requested_confirmations: indexmap::IndexMap<
                    String,
                    crate::core::ToolConfirmation,
                > = Default::default();
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

                    let outcome = dispatch_tool_call(tool.as_ref(), fc, &mut tctx).await?;

                    if let Some(t) = tctx.transfer_to_agent.take() {
                        if !me.disable_transfer { transfer = Some(t); }
                    }
                    if tctx.escalate { escalate = true; }
                    let pending_name = outcome.pending_response_name();
                    let value = match outcome {
                        ToolDispatch::Completed(v) | ToolDispatch::AuthPending(v) => v,
                        ToolDispatch::ConfirmationPending(v, confirmation) => {
                            requested_confirmations.insert(
                                fc.id.clone().unwrap_or_else(|| fc.name.clone()),
                                confirmation,
                            );
                            v
                        }
                    };
                    let will_continue = if tool.is_long_running()
                        || tctx.long_running
                        || pending_name.is_some()
                    {
                        // Either a genuine long-running handle, or the call
                        // is gated on user input (confirmation / auth
                        // consent). Both pause the invocation; the caller
                        // resumes by resubmitting a FunctionResponse.
                        long_running_any = true;
                        long_running_tool_ids.push(
                            fc.id.clone().unwrap_or_else(|| fc.name.clone())
                        );
                        Some(true)
                    } else {
                        None
                    };
                    let response_name = pending_name
                        .map(str::to_string)
                        .unwrap_or_else(|| fc.name.clone());
                    tool_responses.push(
                        FunctionResponse { id: fc.id.clone(), name: response_name, response: value, will_continue, scheduling: None }
                    );
                }

                // Emit a tool-response event.
                let mut tool_event = function_response_event(&me.name, &ctx2.invocation_id, tool_responses.clone());
                if !long_running_tool_ids.is_empty() {
                    tool_event.long_running_tool_ids = Some(long_running_tool_ids);
                }
                if !requested_confirmations.is_empty() {
                    tool_event.actions.requested_tool_confirmations = requested_confirmations;
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
                    // needs interactive consent. Mark the invocation paused
                    // (so ancestor workflow agents stop their pipelines) and
                    // let the caller resume on a follow-up invocation.
                    ctx2.attributes
                        .lock()
                        .insert("invocation.paused".into(), serde_json::Value::Bool(true));
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

/// Build the outgoing request. Returns the request plus, when a
/// `static_instruction` is configured, the resolved dynamic instructions to
/// append to the request *contents* (kept out of the system instruction so
/// the cacheable prefix stays stable).
async fn build_request(
    agent: &LlmAgent,
    ctx: &Arc<InvocationContext>,
) -> Result<(LlmRequest, Option<String>)> {
    let mut req = LlmRequest {
        model: Some(agent.model.name().to_string()),
        cache_config: ctx.run_config.context_cache_config.clone(),
        ..Default::default()
    };

    // Instructions. The static instruction (if any) is appended first and
    // verbatim — no templating, no per-turn re-evaluation.
    let ro = ReadonlyContext::new(ctx.clone());
    if let Some(static_inst) = &agent.static_instruction {
        req.append_system_text(&static_inst.text_concat());
    }
    let mut dynamic = String::new();
    if let Some(inst) = &agent.global_instruction {
        let s = resolve_instruction(inst, &ro).await?;
        if !s.is_empty() {
            dynamic.push_str(&s);
        }
    }
    if let Some(inst) = &agent.instruction {
        let s = resolve_instruction(inst, &ro).await?;
        if !s.is_empty() {
            if !dynamic.is_empty() {
                dynamic.push_str("\n\n");
            }
            dynamic.push_str(&s);
        }
    }
    let mut deferred = None;
    if !dynamic.is_empty() {
        if agent.static_instruction.is_some() {
            deferred = Some(dynamic);
        } else {
            req.append_system_text(&dynamic);
        }
    }

    // Structured output.
    if let Some(schema) = &agent.output_schema {
        req.set_output_schema(schema.clone());
    }

    // Tools.
    let mut tctx = ToolContext::new(ctx.clone());
    for t in &agent.tools {
        t.process_llm_request(&mut req, &mut tctx).await?;
        req.tools_dict.insert(t.name().to_string(), t.clone());
    }
    Ok((req, deferred))
}

/// Static instructions get `{state_key}` templating (mirrors Python, where
/// string instructions are templated and instruction *providers* bypass
/// injection and read state themselves).
async fn resolve_instruction(i: &Instruction, ctx: &ReadonlyContext) -> Result<String> {
    match i {
        Instruction::Static(s) => {
            crate::agents::instructions::inject_session_state(s, ctx).await
        }
        Instruction::Dynamic(f) => f(ctx).await,
    }
}

/// Extract the value to store under `output_key` from a final-response
/// event: the concatenated text parts, JSON-parsed when an `output_schema`
/// is configured. Returns `None` when the event carries no text.
fn output_value(event: &Event, structured: bool) -> Option<serde_json::Value> {
    let text: String = event
        .response
        .content
        .as_ref()?
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    if text.is_empty() {
        return None;
    }
    if structured {
        match serde_json::from_str(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    "output_schema is set but the final response is not valid JSON \
                     ({e}); storing the raw text instead"
                );
                Some(serde_json::Value::String(text))
            }
        }
    } else {
        Some(serde_json::Value::String(text))
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
    agent_name: &str,
) -> Result<Vec<FunctionResponse>> {
    let ids = resumed_tool_call_ids(ctx);
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let events = ctx.session.lock().events.clone();
    let mut responses = Vec::new();
    let mut consumed: Vec<String> = Vec::new();
    for id in ids {
        // Replay is scoped to the agent that owns the pending call: the
        // event that carried the original FunctionCall was authored by it.
        // Ids owned by other agents in the tree are left untouched for
        // their owners to replay.
        let Some(fc) = events
            .iter()
            .filter(|e| e.author == agent_name)
            .flat_map(Event::function_calls)
            .find(|fc| fc.id.as_deref() == Some(id.as_str()))
        else {
            continue;
        };
        let Some(tool) = req.tools_dict.get(&fc.name).cloned() else {
            // The owning agent no longer has the tool (registry changed
            // between pause and resume). Skip rather than fail the whole
            // run; the model proceeds without the replayed result.
            tracing::warn!(
                tool = %fc.name,
                call_id = %id,
                "resumed tool call references a tool this agent no longer registers; skipping replay"
            );
            continue;
        };
        let mut tctx = ToolContext::new(ctx.clone());
        tctx.function_call_id = fc.id.clone();
        let outcome = dispatch_tool_call(tool.as_ref(), &fc, &mut tctx).await?;
        // Consume the id so later agents in this invocation (or later
        // iterations of this one under LoopAgent) don't replay it again —
        // without this, a user-confirmed tool would execute once per
        // same-named registration downstream.
        consumed.push(id);
        let pending_name = outcome.pending_response_name();
        let value = match outcome {
            ToolDispatch::Completed(v)
            | ToolDispatch::AuthPending(v)
            | ToolDispatch::ConfirmationPending(v, _) => v,
        };
        responses.push(FunctionResponse {
            id: fc.id.clone(),
            name: pending_name
                .map(str::to_string)
                .unwrap_or_else(|| fc.name.clone()),
            response: value,
            will_continue: pending_name.is_some().then_some(true),
            scheduling: None,
        });
    }
    consume_resumed_ids(ctx, &consumed);
    Ok(responses)
}

/// Remove replayed call ids from the invocation-wide resume attributes
/// (`auth.resumed_tool_call_ids`, `confirmation.responses`) so they are
/// replayed exactly once per invocation.
fn consume_resumed_ids(ctx: &InvocationContext, consumed: &[String]) {
    if consumed.is_empty() {
        return;
    }
    let mut attrs = ctx.attributes.lock();
    if let Some(v) = attrs.get_mut("auth.resumed_tool_call_ids") {
        if let Ok(mut ids) = serde_json::from_value::<Vec<String>>(v.clone()) {
            ids.retain(|id| !consumed.iter().any(|c| c == id));
            *v = serde_json::to_value(ids).unwrap_or(serde_json::Value::Null);
        }
    }
    if let Some(serde_json::Value::Object(map)) = attrs.get_mut("confirmation.responses") {
        for id in consumed {
            map.remove(id);
        }
    }
}

/// Function-call ids unblocked by the current user event: auth consents
/// (absorbed by `AuthPreprocessor`) plus confirmation decisions (absorbed by
/// `ConfirmationPreprocessor`).
fn resumed_tool_call_ids(ctx: &InvocationContext) -> Vec<String> {
    let attrs = ctx.attributes.lock();
    let mut ids: Vec<String> = attrs
        .get("auth.resumed_tool_call_ids")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    if let Some(map) = attrs
        .get("confirmation.responses")
        .and_then(|v| v.as_object())
    {
        for k in map.keys() {
            if !ids.iter().any(|i| i == k) {
                ids.push(k.clone());
            }
        }
    }
    ids
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

/// Outcome of dispatching one tool call through the gate pipeline
/// (confirmation → auth → run).
enum ToolDispatch {
    /// The tool ran (or was denied / errored); the value is its response.
    Completed(serde_json::Value),
    /// The call needs user confirmation; the value is a serialized
    /// [`crate::core::ConfirmationRequest`] plus the requested
    /// [`crate::core::ToolConfirmation`] for the event's actions map.
    ConfirmationPending(serde_json::Value, crate::core::ToolConfirmation),
    /// The call needs interactive auth consent; the value is the pending
    /// `AuthConfig` payload.
    AuthPending(serde_json::Value),
}

impl ToolDispatch {
    /// The synthetic function-response name for this outcome (the tool's
    /// own name for completed calls is substituted by the caller).
    fn pending_response_name(&self) -> Option<&'static str> {
        match self {
            Self::Completed(_) => None,
            Self::ConfirmationPending(..) => {
                Some(crate::core::REQUEST_CONFIRMATION_FUNCTION_NAME)
            }
            Self::AuthPending(_) => Some(crate::auth::REQUEST_CREDENTIAL_FUNCTION_NAME),
        }
    }
}

/// Look up the user's confirmation decision for `function_call_id`,
/// absorbed by the runner from `adk_request_confirmation` responses.
fn confirmation_response_for(
    ctx: &InvocationContext,
    function_call_id: Option<&str>,
) -> Option<crate::core::ToolConfirmation> {
    let id = function_call_id?;
    let attrs = ctx.attributes.lock();
    let map = attrs.get("confirmation.responses")?;
    serde_json::from_value(map.get(id)?.clone()).ok()
}

/// Dispatch one tool call through the gates:
///
/// 1. **Confirmation** — if the tool requires confirmation and no decision
///    is recorded for this call, defer with a [`ConfirmationRequest`]
///    (the tool is *not* run). A recorded denial yields an error response
///    the model can react to; an approval is injected into
///    [`ToolContext::tool_confirmation`].
/// 2. **Auth** (`feature = "auth"`) — resolve the tool's credential;
///    defer with an `adk_request_credential` payload when interactive
///    consent is needed.
/// 3. **Run** — dispatch the tool; errors become `{"error": ...}` values.
async fn dispatch_tool_call(
    tool: &dyn DynTool,
    fc: &crate::genai_types::FunctionCall,
    tctx: &mut ToolContext,
) -> Result<ToolDispatch> {
    if tool.requires_confirmation(&fc.args) {
        match confirmation_response_for(&tctx.invocation, fc.id.as_deref()) {
            Some(c) if c.confirmed => {
                tctx.tool_confirmation = Some(c);
            }
            Some(_) => {
                return Ok(ToolDispatch::Completed(serde_json::json!({
                    "error": "tool call was rejected by the user"
                })));
            }
            None => {
                let confirmation = crate::core::ToolConfirmation {
                    hint: tool.confirmation_hint(&fc.args),
                    confirmed: false,
                    payload: None,
                };
                let request = crate::core::ConfirmationRequest {
                    original_function_call: fc.clone(),
                    tool_confirmation: confirmation.clone(),
                };
                let value = serde_json::to_value(&request).unwrap_or(serde_json::Value::Null);
                return Ok(ToolDispatch::ConfirmationPending(value, confirmation));
            }
        }
    }

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
                    return Ok(ToolDispatch::AuthPending(value));
                }
                crate::auth::ResolveOutcome::Misconfigured(msg) => {
                    return Ok(ToolDispatch::Completed(serde_json::json!({"error": msg})));
                }
            }
        }
    }

    let value = match tool.run(fc.args.clone(), tctx).await {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error": e.to_string()}),
    };
    Ok(ToolDispatch::Completed(value))
}

/// Builder for [`LlmAgent`].
#[derive(Default)]
pub struct LlmAgentBuilder {
    name: String,
    description: String,
    model: Option<Arc<dyn Model>>,
    instruction: Option<Instruction>,
    global_instruction: Option<Instruction>,
    static_instruction: Option<Content>,
    tools: Vec<Arc<dyn DynTool>>,
    sub_agents: Vec<Arc<dyn BaseAgent>>,
    disable_transfer: bool,
    max_iterations: Option<u32>,
    output_key: Option<String>,
    output_schema: Option<Schema>,
    include_contents: IncludeContents,
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

    /// Cache-stable instruction prefix. Sent verbatim (never templated) at
    /// the very start of the system instruction; when set, the dynamic
    /// `instruction` moves into the request contents so the system prefix
    /// stays byte-identical across turns. Pair with
    /// [`crate::core::ContextCacheConfig`] for explicit context caching.
    #[must_use]
    pub fn static_instruction(mut self, s: impl Into<String>) -> Self {
        self.static_instruction = Some(Content::system_text(s));
        self
    }

    /// Like [`Self::static_instruction`] but accepts arbitrary [`Content`]
    /// (e.g. multimodal parts).
    #[must_use]
    pub fn static_instruction_content(mut self, c: Content) -> Self {
        self.static_instruction = Some(c);
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

    /// Save the agent's final response into session state under this key
    /// (via the final event's `state_delta`). With [`Self::output_schema`],
    /// the stored value is the parsed JSON object rather than raw text.
    #[must_use]
    pub fn output_key(mut self, key: impl Into<String>) -> Self {
        self.output_key = Some(key.into());
        self
    }

    /// Force structured JSON output conforming to `schema`.
    #[must_use]
    pub fn output_schema(mut self, schema: Schema) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Control how much conversation history the model sees.
    #[must_use]
    pub fn include_contents(mut self, ic: IncludeContents) -> Self {
        self.include_contents = ic;
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
            static_instruction: self.static_instruction,
            tools: self.tools,
            sub_agents: self.sub_agents,
            disable_transfer: self.disable_transfer,
            max_iterations: self.max_iterations.unwrap_or(16),
            output_key: self.output_key,
            output_schema: self.output_schema,
            include_contents: self.include_contents,
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

    #[tokio::test]
    async fn output_key_stamps_state_delta_on_final_event() {
        let model = Arc::new(MockModel::new("mock-1"));
        model.push_text("Paris");
        let agent = Arc::new(
            LlmAgent::builder("capitals")
                .model(model.clone() as Arc<dyn Model>)
                .output_key("capital")
                .build()
                .unwrap(),
        );
        let svc: Arc<dyn crate::core::SessionService> = Arc::new(InMemorySessionService::new());
        let ctx = build_ctx(svc, "capital of France?");
        let mut stream = agent.run(ctx.clone()).await.unwrap();
        let mut last = None;
        while let Some(e) = stream.next().await {
            last = Some(e.unwrap());
        }
        let last = last.unwrap();
        assert_eq!(
            last.actions.state_delta.get("capital"),
            Some(&serde_json::json!("Paris"))
        );
        // The in-memory session copy was updated to match.
        let sess = ctx.session.lock();
        let stored = sess.events.iter().find(|e| e.id == last.id).unwrap();
        assert_eq!(
            stored.actions.state_delta.get("capital"),
            Some(&serde_json::json!("Paris"))
        );
    }

    #[tokio::test]
    async fn output_schema_parses_json_and_sets_request_schema() {
        let model = Arc::new(MockModel::new("mock-1"));
        model.push_text(r#"{"city": "Paris", "population": 2100000}"#);
        let agent = Arc::new(
            LlmAgent::builder("extract")
                .model(model.clone() as Arc<dyn Model>)
                .output_key("info")
                .output_schema(
                    crate::genai_types::Schema::object()
                        .property("city", crate::genai_types::Schema::string()),
                )
                .build()
                .unwrap(),
        );
        let svc: Arc<dyn crate::core::SessionService> = Arc::new(InMemorySessionService::new());
        let ctx = build_ctx(svc, "extract");
        let mut stream = agent.run(ctx).await.unwrap();
        let mut last = None;
        while let Some(e) = stream.next().await {
            last = Some(e.unwrap());
        }
        // Stored as parsed JSON, not a string.
        assert_eq!(
            last.unwrap().actions.state_delta.get("info").unwrap()["city"],
            serde_json::json!("Paris")
        );
        // Request carried the response schema + JSON mime type.
        let reqs = model.captured_requests();
        assert!(reqs[0].config.response_schema.is_some());
        assert_eq!(
            reqs[0].config.response_mime_type.as_deref(),
            Some("application/json")
        );
    }

    #[tokio::test]
    async fn include_contents_none_drops_history() {
        let model = Arc::new(MockModel::new("mock-1"));
        model.push_text("ok");
        let agent = Arc::new(
            LlmAgent::builder("stateless")
                .model(model.clone() as Arc<dyn Model>)
                .include_contents(IncludeContents::None)
                .build()
                .unwrap(),
        );
        let svc: Arc<dyn crate::core::SessionService> = Arc::new(InMemorySessionService::new());
        let ctx = build_ctx(svc, "current turn");
        // Pre-populate history that must NOT be sent.
        ctx.session
            .lock()
            .events
            .push(Event::model_text("stateless", "old reply"));
        let mut stream = agent.run(ctx).await.unwrap();
        while let Some(e) = stream.next().await {
            e.unwrap();
        }
        let reqs = model.captured_requests();
        assert_eq!(reqs[0].contents, vec![Content::user_text("current turn")]);
    }

    #[tokio::test]
    async fn static_instruction_pins_system_and_defers_dynamic() {
        let model = Arc::new(MockModel::new("mock-1"));
        model.push_text("ok");
        let agent = Arc::new(
            LlmAgent::builder("cached")
                .model(model.clone() as Arc<dyn Model>)
                .static_instruction("STABLE PREFIX")
                .instruction("dynamic for {user:name}")
                .build()
                .unwrap(),
        );
        let svc: Arc<dyn crate::core::SessionService> = Arc::new(InMemorySessionService::new());
        let ctx = build_ctx(svc, "hi");
        ctx.session
            .lock()
            .state
            .set("user:name", serde_json::json!("Ada"));
        let mut stream = agent.run(ctx).await.unwrap();
        while let Some(e) = stream.next().await {
            e.unwrap();
        }
        let reqs = model.captured_requests();
        // System instruction contains ONLY the static prefix.
        let sys = reqs[0]
            .config
            .system_instruction
            .as_ref()
            .map(|c| c.text_concat())
            .unwrap_or_default();
        assert_eq!(sys, "STABLE PREFIX");
        // The dynamic, templated instruction rides in the contents.
        let last = reqs[0].contents.last().unwrap();
        assert_eq!(last.text_concat(), "dynamic for Ada");
    }

    #[tokio::test]
    async fn static_instruction_is_templated_from_state() {
        let model = Arc::new(MockModel::new("mock-1"));
        model.push_text("ok");
        let agent = Arc::new(
            LlmAgent::builder("templated")
                .model(model.clone() as Arc<dyn Model>)
                .instruction("Speak in {language}. Audience: {audience?}.")
                .build()
                .unwrap(),
        );
        let svc: Arc<dyn crate::core::SessionService> = Arc::new(InMemorySessionService::new());
        let ctx = build_ctx(svc, "hi");
        ctx.session
            .lock()
            .state
            .set("language", serde_json::json!("French"));
        let mut stream = agent.run(ctx).await.unwrap();
        while let Some(e) = stream.next().await {
            e.unwrap();
        }
        let reqs = model.captured_requests();
        let sys = reqs[0]
            .config
            .system_instruction
            .as_ref()
            .map(|c| c.text_concat())
            .unwrap_or_default();
        assert!(sys.contains("Speak in French."), "got: {sys}");
        assert!(sys.contains("Audience: ."), "got: {sys}");
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
