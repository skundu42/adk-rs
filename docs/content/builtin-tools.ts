import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'builtin-tools',
  title: 'Built-in tools & toolsets',
  description:
    'The tools that ship with adk-rs — agent control, Gemini server-side capabilities, memory and artifact access, human-in-the-loop prompts — plus AgentTool and the Toolset trait.',
  srcPath: 'src/tools/builtin.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs ships a small catalogue of ready-made tools in `adk_rs::tools`. Each is a zero-config constructor function returning `Arc<dyn DynTool>`, ready to pass to `LlmAgent::builder().tool(...)`. They fall into three groups: agent-control tools, Gemini server-side capabilities, and service-backed tools for memory and artifacts.',
    },
    { kind: 'h2', text: 'Catalogue' },
    {
      kind: 'table',
      head: ['Constructor', 'Purpose', 'Requires'],
      rows: [
        ['`transfer_to_agent_tool()`', 'Hand control to another agent by name.', 'an agent tree with named sub-agents'],
        ['`exit_loop()`', 'Break out of a `LoopAgent` by escalating.', 'a surrounding `LoopAgent`'],
        ['`google_search_tool()`', 'Google Search grounding, executed on Google’s servers.', 'Gemini model'],
        ['`url_context_tool()`', 'Ground responses in URLs the user mentioned.', 'Gemini 2+ model'],
        ['`built_in_code_execution_tool()`', 'Sandboxed Python, executed on Google’s servers.', 'Gemini model'],
        ['`get_user_choice_tool()`', 'Pause and ask the user to pick an option (HITL).', 'a caller that resumes the run'],
        ['`load_memory_tool()`', 'Search long-term memory on demand.', 'a [memory service](/docs/memory)'],
        ['`preload_memory_tool(max_entries)`', 'Inline relevant memories into the system prompt each turn.', 'a memory service'],
        ['`load_artifacts_tool()`', 'Pull previously-saved artifacts into the conversation.', 'an [artifact service](/docs/artifacts)'],
        ['`AgentTool::wrap(agent)`', 'Call a whole agent as a function.', 'any `Arc<dyn BaseAgent>`'],
      ],
    },
    { kind: 'h2', text: 'Agent-control tools' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'transfer_to_agent_tool() -> Arc<dyn DynTool>',
          desc: 'Declares one required string arg, `agent_name`. Its `run` sets `ToolContext::transfer_to_agent`; the agent then locates the named agent in the tree and streams the rest of the invocation from it. See [multi-agent](/docs/multi-agent).',
        },
        {
          sig: 'exit_loop() -> Arc<dyn DynTool>',
          desc: 'Takes no arguments; sets `ToolContext::escalate = true`, which emits an escalation event and breaks the enclosing `LoopAgent` iteration. See [workflow agents](/docs/workflow-agents).',
        },
      ],
    },
    { kind: 'h2', text: 'Gemini server-side tools' },
    {
      kind: 'p',
      text: '`google_search_tool()`, `url_context_tool()`, and `built_in_code_execution_tool()` are **passive** handles: `declaration()` returns `None`, so the model never sees them as callable functions, and their `run` is never dispatched. Their entire job is done in `process_llm_request`, where they inject the matching wire variant — `Tool::GoogleSearch {}`, `Tool::UrlContext {}`, or `Tool::CodeExecution {}` — into `req.config.tools` (deduplicated across turns).',
    },
    {
      kind: 'callout',
      tone: 'note',
      title: 'These execute on Google’s servers',
      text: 'When the request carries one of these wire tools, the Gemini API itself performs the search, fetches the URLs, or runs the Python — nothing executes in your process. Search citations come back via `LlmResponse::grounding_metadata`; server-side code execution returns `Part::ExecutableCode` and `Part::CodeExecutionResult` parts, with no local executor required. Attached to a non-Gemini model the handles are no-ops or are rejected server-side.',
    },
    { kind: 'h2', text: 'Human-in-the-loop: get_user_choice' },
    {
      kind: 'p',
      text: '`get_user_choice_tool()` is a long-running tool the **model** calls with `{prompt, options: [..]}`. Its `run` sets `ctx.long_running = true` and returns `{"status": "awaiting_user", "prompt": ..., "options": [...]}`; the agent pauses the invocation. Your application displays the prompt and resumes by resubmitting the chosen option as a follow-up `FunctionResponse`. See [cancellation & resume](/docs/cancellation-and-resume).',
    },
    { kind: 'h2', text: 'Memory and artifact tools' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'load_memory_tool() -> Arc<dyn DynTool>',
          desc: 'Declares one required string arg, `query`. Calls `MemoryService::search_memory(app, user, query)` and returns `{"memories": [...]}`. Errors if no memory service is configured.',
        },
        {
          sig: 'preload_memory_tool(max_entries: usize) -> Arc<dyn DynTool>',
          desc: 'Passive (no declaration). In `process_llm_request` it searches memory with the invocation’s user content and appends up to `max_entries` hits to the system prompt as a “Relevant prior context” block. Silently does nothing without a memory service or user content.',
        },
        {
          sig: 'load_artifacts_tool() -> Arc<dyn DynTool>',
          desc: 'Declares a required `artifact_names: string[]` arg. Loads each via `ToolContext::load_artifact` and returns `{"loaded": [{"name", "part"} | {"name", "error": "not found"}]}`.',
        },
      ],
    },
    { kind: 'h2', text: 'AgentTool: an agent as a function' },
    {
      kind: 'p',
      text: '`AgentTool` wraps any `Arc<dyn BaseAgent>` as a callable tool named after the agent, declaring one required string arg, `request`. Unlike `transfer_to_agent` — which hands over the whole invocation — `AgentTool` runs the child as a **sub-invocation** sharing the parent’s session and services, accumulates the text of every event the child emits, and returns it as `{"text": ...}` (plus an `error` field if the child reported one). Control returns to the parent, which can keep reasoning over the result.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Delegating to a specialist',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::tools::AgentTool;
use std::sync::Arc;

let researcher = Arc::new(
    LlmAgent::builder("researcher")
        .description("Finds and summarises sources on a topic.")
        .model(model.clone())
        .build()?,
);

let writer = LlmAgent::builder("writer")
    .model(model)
    .instruction("Call the researcher when you need facts, then write the answer.")
    .tool(AgentTool::wrap(researcher))
    // or: AgentTool::wrap_with_description(researcher, "Research assistant")
    .build()?;`,
    },
    { kind: 'h2', text: 'Toolsets: dynamic collections' },
    {
      kind: 'p',
      text: 'A `Toolset` is a lazy supplier of tools for sources whose tool list is only known at runtime — MCP servers, OpenAPI specs. The trait has two methods: `async fn list_tools(&self, ctx: &ReadonlyContext) -> Result<Vec<Arc<dyn DynTool>>>` and an optional `async fn shutdown(&self)` for backends holding OS resources. `StaticToolset` is the trivial implementation over a fixed `Vec` (`StaticToolset::new(tools)`, `push(tool)`); [`McpToolset`](/docs/mcp) is the canonical dynamic one.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Tools overview](/docs/tools-overview) — the `DynTool` trait and dispatch loop.',
        '[Memory](/docs/memory) — the service behind `load_memory` / `preload_memory`.',
        '[Code execution](/docs/code-execution) — local and Docker executors, vs. the Gemini server-side sandbox.',
        '[MCP integration](/docs/mcp) — toolsets backed by external servers.',
      ],
    },
  ],
};
