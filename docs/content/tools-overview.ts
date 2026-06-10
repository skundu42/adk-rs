import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'tools-overview',
  title: 'Tools overview',
  description:
    'The DynTool trait that every tool implements, the ToolContext passed to each call, and the dispatch loop that connects model function calls to your code.',
  srcPath: 'src/core/tool_object.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'A tool in adk-rs is anything implementing the `DynTool` trait: a name, a description, a JSON-Schema declaration the model sees, and an async `run` that receives JSON args plus a mutable `ToolContext`. Through that context a tool can do far more than return a value — it can mutate session state, save artifacts, transfer control to another agent, or pause the whole invocation.',
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'The public alias `adk_rs::tools::Tool` (also re-exported at the crate root as `adk_rs::Tool`) **is** `adk_rs::core::DynTool` — one trait, two names. The trait lives in `core` so `LlmRequest` can carry a `tools_dict: HashMap<String, Arc<dyn DynTool>>` without a circular dependency.',
    },
    { kind: 'h2', text: 'The DynTool trait' },
    {
      kind: 'api',
      entries: [
        { sig: 'fn name(&self) -> &str', desc: 'Tool name; must match the `FunctionCall.name` the model emits.' },
        { sig: 'fn description(&self) -> &str', desc: 'Human description shown to the model.' },
        {
          sig: 'fn is_long_running(&self) -> bool',
          desc: 'Default `false`. When `true`, the agent emits the tool’s `FunctionResponse` with `will_continue: Some(true)` and pauses; the caller resumes by resubmitting the final result later. See [cancellation & resume](/docs/cancellation-and-resume).',
        },
        {
          sig: 'fn auth_config(&self) -> Option<&AuthConfig>',
          desc: 'Default `None`. When `Some`, the runner resolves the credential **before** calling `run` and injects it into `ToolContext::auth_credential`; if interactive consent is needed it emits an `adk_request_credential` event instead of dispatching. See [Auth](/docs/auth).',
        },
        {
          sig: 'fn requires_confirmation(&self, args: &Value) -> bool',
          desc: 'Default `false`. When `true` for a given call, the agent pauses with an `adk_request_confirmation` request instead of dispatching — human-in-the-loop per call, so you can confirm only destructive operations. See [tool confirmation](/docs/tool-confirmation).',
        },
        {
          sig: 'fn confirmation_hint(&self, args: &Value) -> String',
          desc: 'Hint shown to the user when confirmation is requested. Defaults to a generic message naming the tool.',
        },
        {
          sig: 'fn declaration(&self) -> Option<FunctionDeclaration>',
          desc: 'JSON-Schema declaration of the parameters. Return `None` for passive tools (e.g. Gemini server-side built-ins) that should not be advertised to the model.',
        },
        {
          sig: 'async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value>',
          desc: 'Execute with JSON args; return a JSON value that becomes the `FunctionResponse` payload.',
        },
        {
          sig: 'async fn process_llm_request(&self, req: &mut LlmRequest, ctx: &mut ToolContext) -> Result<()>',
          desc: 'Hook called before every model call. Default: append `declaration()` into `req.config.tools`. Passive tools override this to inject wire-level config (search grounding, memory preloads) instead.',
        },
      ],
    },
    { kind: 'h2', text: 'ToolContext: what a tool can do' },
    {
      kind: 'p',
      text: 'Every `run` call receives a fresh `ToolContext` built around the shared `InvocationContext` (session, services, run config). Fields a tool sets on the context become **actions** the agent applies after the call returns:',
    },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Effect'],
      rows: [
        ['`invocation`', '`Arc<InvocationContext>`', 'Read access to app name, user id, session, services, and the cancellation token.'],
        ['`function_call_id`', '`Option<String>`', 'Id of the `FunctionCall` being served (matches `FunctionCall::id`).'],
        ['`state_delta`', '`StateDelta`', 'Key/value writes merged into [session state](/docs/sessions-and-state) when the event is appended.'],
        ['`artifact_delta`', '`IndexMap<String, u64>`', 'Filename → new version, populated by `save_artifact`.'],
        ['`skip_summarization`', '`bool`', 'When a tool sets it, the agent ends the turn right after the tool-response event — the tool response becomes the final answer, with no further model call.'],
        ['`transfer_to_agent`', '`Option<String>`', 'Hands the rest of the invocation to the named agent in the tree.'],
        ['`escalate`', '`bool`', 'Unwinds escalation — e.g. breaks a `LoopAgent` iteration.'],
        ['`long_running`', '`bool`', 'Marks this call as a long-running operation handle, pausing the invocation.'],
        ['`auth_credential`', '`Option<AuthCredential>`', 'Resolved credential, injected by the runner when the tool declared an `auth_config()`.'],
        ['`tool_confirmation`', '`Option<ToolConfirmation>`', 'The user’s approval (with any payload), set before `run` when confirmation was required and granted.'],
      ],
    },
    {
      kind: 'p',
      text: 'Tool-written `state_delta` and `artifact_delta` ride on the tool-response event’s `actions`, so the session service persists them when the event is appended.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'async fn save_artifact(&mut self, filename: &str, part: Part) -> Result<u64>',
          desc: 'Persists a part via the configured [artifact service](/docs/artifacts), records the new version in `artifact_delta`, and returns it. Errors if no artifact service is configured.',
        },
        {
          sig: 'async fn load_artifact(&self, filename: &str, version: Option<u64>) -> Result<Option<Part>>',
          desc: 'Loads an artifact (latest version when `None`).',
        },
        {
          sig: 'fn with_function_call_id(self, id: impl Into<String>) -> Self',
          desc: 'Builder-style setter for `function_call_id`.',
        },
      ],
    },
    { kind: 'h2', text: 'The dispatch loop inside LlmAgent' },
    {
      kind: 'p',
      text: 'Each turn of an [`LlmAgent`](/docs/llm-agent) runs the same cycle, capped by `max_iterations` and the run config’s `max_llm_calls`:',
    },
    {
      kind: 'list',
      ordered: true,
      items: [
        '**Preprocess.** Every attached tool gets `process_llm_request(&mut req, ...)` — most append their declaration; passive tools inject config — and is registered in `req.tools_dict` under its name.',
        '**Call the model.** The response becomes an [event](/docs/events); missing `FunctionCall` ids are synthesized so resume and replay stay stable.',
        '**No function calls?** The event is the final response (after optional [code execution](/docs/code-execution) of any `ExecutableCode` parts).',
        '**Dispatch.** Each call is looked up in `tools_dict` and pushed through the gate pipeline — confirmation → auth → `run`. The run itself is wrapped by the agent’s [callbacks](/docs/callbacks-and-plugins): `before_tool` may rewrite the args or short-circuit with a ready-made result, `after_tool` may rewrite the result, and `on_tool_error` may recover a failed call (otherwise `{"error": ...}` is surfaced to the model). Unknown names error with `ToolError::Unknown`.',
        '**Collect actions.** `transfer_to_agent` reroutes the invocation to a sub-agent — an unknown or unreachable target produces a recoverable `{"error": "unknown agent ..."}` tool response instead of aborting the invocation; `escalate` emits an escalation marker and stops; long-running or consent-gated calls get `will_continue: Some(true)` and pause the invocation.',
        '**Loop.** Tool responses are appended to `req.contents` as a `Role::Tool` turn and the cycle repeats.',
      ],
    },
    { kind: 'h2', text: 'Where tools come from' },
    {
      kind: 'list',
      items: [
        '**[`#[tool]` macro](/docs/function-tools)** — annotate an async fn; get a typed, schema-deriving `Arc<dyn Tool>` constructor for free.',
        '**[`FunctionTool`](/docs/function-tools)** — wrap any async closure explicitly, with manual name/description/schema.',
        '**[Built-ins](/docs/builtin-tools)** — `transfer_to_agent`, `exit_loop`, `google_search`, `load_memory`, `load_artifacts`, and friends.',
        '**[`AgentTool`](/docs/builtin-tools)** — expose a whole agent as a callable tool.',
        '**[OpenAPI tools](/docs/openapi-tools)** — one `RestApiTool` per operation in an OpenAPI 3.x spec.',
        '**[MCP toolset](/docs/mcp)** — every tool advertised by a Model Context Protocol server, over stdio or streamable HTTP.',
      ],
    },
    {
      kind: 'p',
      text: 'For dynamic sources there is also the `Toolset` trait (`async fn list_tools(&self, ctx: &ReadonlyContext)` plus an optional `shutdown`), with `StaticToolset` as the trivial fixed-list implementation. `McpToolset` and the OpenAPI generator build on it.',
    },
    { kind: 'h2', text: 'Attaching tools to an agent' },
    {
      kind: 'code',
      lang: 'rust',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::tools::{exit_loop, transfer_to_agent_tool};

let agent = LlmAgent::builder("orchestrator")
    .model(model)
    .instruction("Coordinate the team; exit when done.")
    .tool(transfer_to_agent_tool())
    .tool(exit_loop())
    .tools(my_openapi_tools) // any IntoIterator<Item = Arc<dyn DynTool>>
    .build()?;`,
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'You rarely need to attach `transfer_to_agent_tool()` by hand: an `LlmAgent` that declares `sub_agents` auto-registers it (unless `disable_transfer` is set). Manual attachment is only for transfer without declared sub-agents.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Function tools & #[tool]](/docs/function-tools) — the ergonomic path to custom tools.',
        '[Tool confirmation](/docs/tool-confirmation) — the human-in-the-loop gate in depth.',
        '[Auth](/docs/auth) — credential resolution for authenticated tools.',
        '[Multi-agent](/docs/multi-agent) — transfer and escalation across an agent tree.',
      ],
    },
  ],
};
