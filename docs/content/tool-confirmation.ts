import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'tool-confirmation',
  title: 'Tool confirmation (HITL)',
  description:
    'Gate dangerous tool calls behind explicit human approval with the adk_request_confirmation pause-and-resume flow.',
  srcPath: 'src/core/tool_confirmation.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'A tool marked `require_confirmation` never runs on the model\'s say-so alone. Instead of dispatching, the agent emits a synthetic `adk_request_confirmation` function response and pauses the invocation; your application shows the hint to a human, and resubmits their decision as a function response. Only an explicit approval lets the original call execute — exactly once.',
    },
    { kind: 'h2', text: 'The flow, step by step' },
    {
      kind: 'list',
      ordered: true,
      items: [
        '**Register the gate.** Build the tool with `FunctionTool::require_confirmation(true)` (optionally `with_confirmation_hint`), or implement `DynTool::requires_confirmation` yourself — it receives the call `args`, so you can confirm only destructive parameter combinations.',
        '**The model calls the tool.** The agent\'s dispatch pipeline (confirmation → auth → run) sees the gate and does **not** run the tool. It emits a `FunctionResponse` named `adk_request_confirmation` (`REQUEST_CONFIRMATION_FUNCTION_NAME`) with the same function-call `id`, `will_continue: Some(true)`, and a serialized `ConfirmationRequest` as the response value.',
        '**The invocation pauses.** The tool-response event lists the call id in `long_running_tool_ids` and in `actions.requested_tool_confirmations` (id → `ToolConfirmation`), and the agent stops issuing turns.',
        '**Show the hint, collect a decision.** Your UI presents `ToolConfirmation.hint` (and the `originalFunctionCall` args) to the human.',
        '**Resubmit the decision.** Send a new user `Content` containing a `FunctionResponse` with the *same* `id`, name `adk_request_confirmation`, and a `ToolConfirmation` value (`confirmed: true/false`, optional `payload`).',
        '**The runner absorbs and replays.** `ConfirmationPreprocessor::process_event` extracts decisions from the user event into a `ConfirmationOutcome`; the owning agent replays the original call — running the tool if confirmed (the decision is injected into `ToolContext::tool_confirmation`), or returning `{"error": "tool call was rejected by the user"}` to the model if denied.',
      ],
    },
    { kind: 'h2', text: 'Types' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub const REQUEST_CONFIRMATION_FUNCTION_NAME: &str = "adk_request_confirmation"',
          desc: 'Name of the synthetic function response used to request — and answer — a confirmation.',
        },
        {
          sig: 'struct ToolConfirmation { hint: String, confirmed: bool, payload: Option<Value> }',
          desc: 'One decision (or request). `payload` carries optional structured data the user attached, e.g. an amended parameter set; it reaches the tool via `ToolContext::tool_confirmation`.',
        },
        {
          sig: 'struct ConfirmationRequest { original_function_call: FunctionCall, tool_confirmation: ToolConfirmation }',
          desc: 'Payload of the pending response. Serialized in camelCase (`originalFunctionCall`, `toolConfirmation`) for adk-web compatibility.',
        },
        {
          sig: 'ConfirmationPreprocessor::process_event(&self, event: &Event) -> ConfirmationOutcome',
          desc: 'Runner-side absorber: walks a user event\'s function responses for `adk_request_confirmation` entries and decodes the decisions into `ConfirmationOutcome { responses: IndexMap<String, ToolConfirmation> }`.',
        },
        {
          sig: 'FunctionTool::require_confirmation(self, yes: bool) -> Self',
          desc: 'Gate every call to this tool behind confirmation.',
        },
        {
          sig: 'FunctionTool::with_confirmation_hint(self, hint: impl Into<String>) -> Self',
          desc: 'Custom hint. Default: ``Approve execution of tool `{name}`?``',
        },
        {
          sig: 'DynTool::requires_confirmation(&self, args: &Value) -> bool',
          desc: 'Trait-level hook (default `false`). `args` lets implementations decide per call.',
        },
      ],
    },
    { kind: 'h2', text: 'Wire shape' },
    {
      kind: 'p',
      text: 'The pending request the agent emits is a `FunctionResponse` part whose `response` value is the `ConfirmationRequest`:',
    },
    {
      kind: 'code',
      lang: 'json',
      title: 'response value of the adk_request_confirmation FunctionResponse',
      code: `{
  "originalFunctionCall": {
    "id": "call-1",
    "name": "transfer_money",
    "args": { "amount": 100 }
  },
  "toolConfirmation": {
    "hint": "Approve this transfer?",
    "confirmed": false
  }
}`,
    },
    {
      kind: 'p',
      text: 'The answer is a user-authored `FunctionResponse` with the same id. The preprocessor accepts a bare `ToolConfirmation`, or one wrapped as `{"toolConfirmation": {...}}` or `{"response": {...}}`. Responses without a function-call id are dropped with a warning.',
    },
    { kind: 'h2', text: 'End-to-end example' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Gate, pause, approve',
      code: `use adk_rs::core::{RunConfig, ToolContext, REQUEST_CONFIRMATION_FUNCTION_NAME};
use adk_rs::genai_types::{Content, FunctionResponse, Part, Role, Schema};
use adk_rs::tools::FunctionTool;
use serde_json::Value;
use std::sync::Arc;

let transfer = FunctionTool::from_async(
    "transfer_money",
    "Transfer money between accounts",
    Some(Schema::object().property("amount", Schema::number()).require("amount")),
    |args: Value, _ctx: &mut ToolContext| async move {
        Ok(serde_json::json!({ "ok": true, "transferred": args["amount"] }))
    },
)
.require_confirmation(true)
.with_confirmation_hint("Approve this transfer?");

// Turn 1: the model calls transfer_money; the run pauses instead.
let mut events = runner.run("user", Some(&session_id), "send $100").await?;
while let Some(event) = events.next().await {
    let event = event?;
    for (call_id, confirmation) in &event.actions.requested_tool_confirmations {
        println!("approval needed for {call_id}: {}", confirmation.hint);
    }
}

// Turn 2: resubmit the human's decision with the SAME call id.
let approval = Content {
    role: Role::User,
    parts: vec![Part::FunctionResponse(FunctionResponse {
        id: Some("call-1".into()),
        name: REQUEST_CONFIRMATION_FUNCTION_NAME.into(),
        response: serde_json::json!({ "confirmed": true }),
        will_continue: None,
        scheduling: None,
    })],
};
let events = runner
    .run_with("user", Some(&session_id), approval, RunConfig::default())
    .await?;
// transfer_money now runs exactly once; a denial would instead surface
// {"error": "tool call was rejected by the user"} to the model.`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'With `Runner::builder().resumable(true)`, answer via `Runner::resume(...)` instead of a fresh `run_with` turn — workflow pipelines then continue from their checkpoint instead of re-running earlier steps. Either way, replay is scoped to the agent that authored the call and each decision is consumed exactly once, even if a downstream agent registers a same-named tool. See [Cancellation & resume](/docs/cancellation-and-resume).',
    },
    { kind: 'h2', text: 'MCP tools' },
    {
      kind: 'p',
      text: 'MCP servers expose tools you did not write, so the gate lives on the toolset: `McpToolset::with_confirmation_policy` takes a `ConfirmationPolicy` — `None` (default), `All`, or `Named(HashSet<String>)` for a subset of discovered tools. Set it before the first `list_tools` call, since discovered tools are cached. See [MCP](/docs/mcp).',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Cancellation & resume](/docs/cancellation-and-resume) — what pauses an invocation and how `resume` works.',
        '[Function tools](/docs/function-tools) — the rest of the `FunctionTool` surface.',
        '[MCP](/docs/mcp) — confirmation policies for discovered toolsets.',
      ],
    },
  ],
};
