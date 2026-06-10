import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'llm-agent',
  title: 'LlmAgent',
  description:
    'The LLM-powered agent: its builder, instruction templating, conversation history control, and the LLM-to-tool loop.',
  srcPath: 'src/agents/llm_agent.rs',
  blocks: [
    {
      kind: 'lede',
      text: '`LlmAgent` is the workhorse of adk-rs: an agent that sends the session history to a model, dispatches any function calls the model makes through a gate pipeline (confirmation → auth → run), feeds the results back, and repeats until the model produces a final answer.',
    },
    { kind: 'h2', text: 'Building one' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Minimal construction',
      code: `use std::sync::Arc;
use adk_rs::agents::LlmAgent;
use adk_rs::providers::gemini::Gemini;

let agent = LlmAgent::builder("assistant")
    .description("Answers questions about the product.")
    .model(Arc::new(Gemini::from_env("gemini-2.5-flash")?))
    .instruction("You are a concise, factual assistant.")
    .build()?;`,
    },
    {
      kind: 'p',
      text: '`LlmAgent::builder(name)` returns an `LlmAgentBuilder`. `build()` fails if `name` is empty or no model was set. The crate exports `adk_rs::agents::DEFAULT_MODEL` (`"gemini-2.5-flash"`) as a convenient model id constant, but the builder always requires an explicit `.model(...)`.',
    },
    { kind: 'h2', text: 'Builder reference' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub fn description(self, d: impl Into<String>) -> Self',
          desc: 'Human-readable description, shown to a parent model when deciding whether to delegate to this agent.',
        },
        {
          sig: 'pub fn model(self, m: Arc<dyn Model>) -> Self',
          desc: 'The provider model (required). Any [`Model`](/docs/models) implementation: Gemini, Anthropic, OpenAI-compatible, or your own.',
        },
        {
          sig: 'pub fn instruction(self, s: impl Into<String>) -> Self',
          desc: 'Static system instruction. Templated with `{key}` placeholders against session state on every turn (see below).',
        },
        {
          sig: 'pub fn instruction_dyn(self, p: InstructionProvider) -> Self',
          desc: 'Dynamic instruction: an async function `Fn(&ReadonlyContext) -> BoxFuture<Result<String>>` evaluated per turn. Bypasses templating — the provider reads state itself.',
        },
        {
          sig: 'pub fn global_instruction(self, s: impl Into<String>) -> Self',
          desc: 'Static instruction prefixed before `instruction` (separated by a blank line). Also templated.',
        },
        {
          sig: 'pub fn static_instruction(self, s: impl Into<String>) -> Self',
          desc: 'Cache-stable instruction prefix. Sent verbatim — never templated or re-evaluated — so the system instruction stays byte-identical across turns; when set, the dynamic `instruction` rides in the request *contents* instead. Pair with [context caching](/docs/context-caching).',
        },
        {
          sig: 'pub fn static_instruction_content(self, c: Content) -> Self',
          desc: 'Like `static_instruction` but accepts arbitrary `Content` (e.g. multimodal parts).',
        },
        {
          sig: 'pub fn tool(self, t: Arc<dyn DynTool>) -> Self',
          desc: 'Registers one [tool](/docs/tools-overview).',
        },
        {
          sig: 'pub fn tools(self, ts: impl IntoIterator<Item = Arc<dyn DynTool>>) -> Self',
          desc: 'Registers many tools at once (e.g. the output of an [OpenAPI toolset](/docs/openapi-tools)).',
        },
        {
          sig: 'pub fn sub_agent(self, a: Arc<dyn BaseAgent>) -> Self',
          desc: 'Registers a child agent. Call repeatedly for multiple children. See [Multi-agent systems](/docs/multi-agent).',
        },
        {
          sig: 'pub fn disable_transfer(self, yes: bool) -> Self',
          desc: 'When true, `transfer_to_agent` requests raised by tools are ignored and the agent keeps control.',
        },
        {
          sig: 'pub fn max_iterations(self, n: u32) -> Self',
          desc: 'Caps iterations of the LLM↔tool loop within a single run. Default: **16**.',
        },
        {
          sig: 'pub fn output_key(self, key: impl Into<String>) -> Self',
          desc: 'Saves the agent’s final response into session state under this key, via the final event’s `state_delta`. With `output_schema`, the stored value is the parsed JSON rather than raw text.',
        },
        {
          sig: 'pub fn output_schema(self, schema: Schema) -> Self',
          desc: 'Forces structured JSON output conforming to `schema` (sets the request’s response schema and `application/json` mime type). See [Structured output](/docs/structured-output).',
        },
        {
          sig: 'pub fn include_contents(self, ic: IncludeContents) -> Self',
          desc: 'Controls how much conversation history the model sees (see below).',
        },
        {
          sig: 'pub fn code_executor(self, ex: Arc<dyn CodeExecutor>) -> Self',
          desc: '*(feature `code-exec`)* Executes `ExecutableCode` parts emitted by the model and feeds back `CodeExecutionResult` parts. See [Code execution](/docs/code-execution).',
        },
        {
          sig: 'pub fn build(self) -> Result<LlmAgent>',
          desc: 'Validates (non-empty name, model present) and constructs the agent.',
        },
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'The built agent exposes read accessors `name()`, `description()`, `tools()` and `model()`. Lifecycle callbacks are not registered on the builder in v0.3.0 — runtime interception goes through [plugins](/docs/callbacks-and-plugins).',
    },
    { kind: 'h2', text: 'Conversation history: IncludeContents' },
    {
      kind: 'table',
      head: ['Variant', 'Effect'],
      rows: [
        [
          '`IncludeContents::Default`',
          'Full session history is sent (assembled through [event compaction](/docs/event-compaction) when summaries exist), followed by the current user turn.',
        ],
        [
          '`IncludeContents::None`',
          'No history: only the system instruction and the current turn’s user content are sent. Useful for stateless steps in [workflow pipelines](/docs/workflow-agents).',
        ],
      ],
    },
    { kind: 'h2', text: 'Instruction templating' },
    {
      kind: 'p',
      text: 'Static instructions (set via `instruction` or `global_instruction`) pass through `adk_rs::agents::inject_session_state`, which replaces `{placeholder}` references before each model call. Dynamic providers and `static_instruction` bypass injection entirely.',
    },
    {
      kind: 'table',
      head: ['Syntax', 'Meaning'],
      rows: [
        ['`{key}`', 'Replaced with the session-state value for `key`. **Errors** if the key is missing.'],
        ['`{key?}`', 'Optional: replaced with the empty string when missing.'],
        [
          '`{app:key}` / `{user:key}` / `{temp:key}`',
          'Prefixed state keys, matching the [state scopes](/docs/sessions-and-state).',
        ],
        [
          '`{artifact.name}`',
          'Replaced with the named artifact’s content (text parts verbatim, other parts as JSON). Requires an [artifact service](/docs/artifacts); errors if the artifact is missing unless suffixed with `?`.',
        ],
        [
          'anything else',
          'Bodies that are not valid state names — `{"a": 1}`, `{1, 2}`, `{ }` — are left untouched, so JSON snippets in instructions survive.',
        ],
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `let agent = LlmAgent::builder("support")
    .model(model)
    .instruction("Speak in {language}. Customer tier: {user:tier?}.")
    .build()?;`,
    },
    { kind: 'h2', text: 'The LLM↔tool loop' },
    {
      kind: 'list',
      ordered: true,
      items: [
        'Build the request: system instruction (static prefix + resolved dynamic part), output schema, registered tools, then the history per `include_contents` and the current user content.',
        'Check cancellation and the `RunConfig::max_llm_calls` budget, then call the model.',
        'Persist the model event into the session and inspect it. **No function calls?** It is the final response — `output_key` stamps the text (or parsed JSON) into the event’s `state_delta`, the event is yielded, and the stream ends.',
        'Otherwise dispatch every call through the gate pipeline: [tool confirmation](/docs/tool-confirmation) first, then [auth](/docs/auth) resolution, then the tool’s `run`. Tool errors become `{"error": ...}` values the model can react to.',
        'Yield a tool-response event, then act on side effects: a `transfer_to_agent` hands the rest of the invocation to the named sub-agent; `escalate` emits a marker event and stops; a long-running or consent-gated call pauses the invocation for a later [resume](/docs/cancellation-and-resume).',
        'Append the assistant turn and tool responses to the request contents and loop — up to `max_iterations` times, after which a fail-safe event with `error_code: "MAX_ITERATIONS"` is emitted.',
      ],
    },
    {
      kind: 'callout',
      tone: 'warn',
      text: 'Two budgets apply: `max_iterations` bounds the loop *within one agent run*, while `RunConfig::max_llm_calls` bounds model calls across the *whole invocation*, including sub-agents — exceeding the latter is an error, not a graceful event.',
    },
    { kind: 'h2', text: 'Worked example' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'A tool-using agent with structured state output',
      code: `use std::sync::Arc;
use adk_rs::agents::{IncludeContents, LlmAgent};
use adk_rs::genai_types::Schema;
use adk_rs::providers::gemini::Gemini;

let model = Arc::new(Gemini::from_env("gemini-2.5-flash")?);

let extractor = LlmAgent::builder("extractor")
    .description("Extracts structured facts from a support ticket.")
    .model(model)
    .instruction("Extract the customer's issue from the ticket. Locale: {user:locale?}")
    .include_contents(IncludeContents::None)   // stateless pipeline step
    .max_iterations(4)
    .output_schema(
        Schema::object()
            .property("summary", Schema::string())
            .property("severity", Schema::string()),
    )
    .output_key("ticket_info")                 // parsed JSON lands in state
    .build()?;`,
    },
    {
      kind: 'p',
      text: 'A downstream agent in the same [SequentialAgent](/docs/workflow-agents) can now reference `{ticket_info}` in its instruction, or a [callback-free plugin](/docs/callbacks-and-plugins) can read it from the session state.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Tools overview](/docs/tools-overview) — the `Tool`/`DynTool` contract and `FunctionTool`.',
        '[Structured output](/docs/structured-output) — `output_schema` in depth.',
        '[Multi-agent systems](/docs/multi-agent) — `sub_agent`, transfer, and `AgentTool`.',
        '[Context caching](/docs/context-caching) — why `static_instruction` exists.',
      ],
    },
  ],
};
