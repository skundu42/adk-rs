import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'multi-agent',
  title: 'Multi-agent systems',
  description:
    'LLM-driven delegation with transfer_to_agent, agents wrapped as tools with AgentTool, and how events are attributed across an agent tree.',
  srcPath: 'src/agents/llm_agent.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs offers two ways for agents to use other agents: **transfer**, where a coordinator hands the rest of the conversation to a specialist, and **agent-as-a-tool**, where a child agent is invoked like a function and the parent keeps control. Both build on the same `BaseAgent` tree.',
    },
    { kind: 'h2', text: 'LLM-driven delegation: transfer_to_agent' },
    {
      kind: 'p',
      text: 'A coordinator `LlmAgent` registers specialists with `.sub_agent(...)`, which auto-registers the `transfer_to_agent` built-in tool (`adk_rs::tools::transfer_to_agent_tool()`). The model sees a function declaration with one required parameter, `agent_name`, plus the description *“Transfer control to another agent by name…”* — and calls it when a request is better handled elsewhere.',
    },
    {
      kind: 'p',
      text: 'The flow, step by step: the model emits a `FunctionCall` for `transfer_to_agent`; the tool sets `transfer_to_agent` on its `ToolContext`; after the tool-response event is yielded, the `LlmAgent` resolves the name — its own subtree first (via `find_agent`), then the whole tree from `InvocationContext.root_agent` (set by the [Runner](/docs/runner)), so siblings and ancestors are reachable too; self-transfer is rejected. It then runs the target with the **same** `InvocationContext` and streams the target’s events as the remainder of the invocation. The coordinator does not regain control that turn.',
    },
    {
      kind: 'list',
      items: [
        'The target shares the session, so it sees the full conversation including the user request and the transfer call.',
        'An unknown `agent_name` does not abort the run: the model gets a recoverable tool response — `{"error": "unknown agent <name>; transfer not performed"}` — and the run continues. Names should still be unique and stable within the tree.',
        '`.disable_transfer(true)` on the builder makes the agent ignore transfer requests — and suppresses the auto-registration of the transfer tool and the sub-agent roster in the system instruction.',
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      title: 'Auto-injected tool and roster',
      text: 'When sub-agents are declared (and transfer is not disabled), the transfer tool is auto-registered and a roster of sub-agent names and descriptions is appended to the system instruction — no manual `.tool(transfer_to_agent_tool())` or hand-written agent list needed. Manual registration is only required for transfer **without** declared sub-agents.',
    },
    { kind: 'h2', text: 'Coordinator / specialist example' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'A router with two specialists',
      code: `use std::sync::Arc;
use adk_rs::agents::LlmAgent;
use adk_rs::providers::gemini::Gemini;

let model = Arc::new(Gemini::from_env("gemini-2.5-flash")?);

let greeter = Arc::new(
    LlmAgent::builder("greeter")
        .model(model.clone())
        .description("Greets the user warmly.")
        .instruction("Reply with a friendly greeting.")
        .build()?,
);

let task_executor = Arc::new(
    LlmAgent::builder("task_executor")
        .model(model.clone())
        .description("Executes user tasks step by step.")
        .instruction("Carry out the requested task.")
        .build()?,
);

// Declaring sub-agents auto-registers transfer_to_agent and
// advertises each specialist's name and description to the model.
let coordinator = LlmAgent::builder("coordinator")
    .model(model)
    .description("Routes each request to the right specialist.")
    .instruction("Route each request to the best-suited specialist.")
    .sub_agent(greeter)
    .sub_agent(task_executor)
    .build()?;`,
    },
    {
      kind: 'p',
      text: 'Pass `coordinator` to the [runner](/docs/runner) as the root agent. Each sub-agent’s `description()` is what *you* surface to the model — keep it short and discriminative, since routing quality depends on it.',
    },
    { kind: 'h2', text: 'Agent as a tool: AgentTool' },
    {
      kind: 'p',
      text: '`AgentTool` (in `src/tools/agent_tool.rs`) wraps any `Arc<dyn BaseAgent>` as a `DynTool`. Unlike transfer, the parent **stays in charge**: the model calls the child like a function, the child runs to completion, and its output comes back as a tool result the parent can reason over and combine with other results — including calling several agent-tools in one turn.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub fn wrap(agent: Arc<dyn BaseAgent>) -> Arc<Self>',
          desc: 'Wraps the agent under its own name, reusing the agent’s `description()` as the tool description.',
        },
        {
          sig: 'pub fn wrap_with_description(agent: Arc<dyn BaseAgent>, description: impl Into<String>) -> Arc<Self>',
          desc: 'Same, but overrides the description shown to the calling model.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'The generated declaration takes one required string parameter, `request` — the task to delegate. On invocation, `AgentTool` builds a sub-`InvocationContext` that shares the caller’s session, services, cancellation token and LLM-call budget, with a derived invocation id of the form `{parent_invocation}.sub.{agent_name}`. It then drains the child’s event stream and returns `{"text": "..."}` — the newline-joined text of every event the child emitted — adding an `"error"` field if any child event carried an error message.',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `use adk_rs::tools::AgentTool;

let summarizer = Arc::new(
    LlmAgent::builder("summarizer")
        .model(model.clone())
        .description("Summarizes any text it is given.")
        .instruction("Summarize the request in three bullet points.")
        .build()?,
);

let parent = LlmAgent::builder("analyst")
    .model(model)
    .instruction("Use the summarizer tool when the user pastes long text.")
    .tool(AgentTool::wrap(summarizer))
    .build()?;`,
    },
    { kind: 'h2', text: 'Transfer vs. AgentTool' },
    {
      kind: 'table',
      head: ['', '`transfer_to_agent`', '`AgentTool`'],
      rows: [
        ['Control', 'Hands the rest of the invocation to the target.', 'Parent keeps the loop; child is one function call.'],
        ['Output', 'Target’s events stream directly to the caller.', 'Child events are collapsed into a single JSON result.'],
        ['History', 'Target sees the full shared session.', 'Child shares the session but is driven by the `request` argument.'],
        ['Use when', 'The specialist should own the conversation from here.', 'You need a sub-result the parent will post-process.'],
      ],
    },
    { kind: 'h2', text: 'Event attribution across the tree' },
    {
      kind: 'p',
      text: 'Every [`Event`](/docs/events) records its origin: `author` is the emitting agent’s name (or `"user"`), `invocation_id` ties it to a run, and `branch` labels concurrent fan-outs — a [`ParallelAgent`](/docs/workflow-agents) stamps `"{parent}.{index}"` onto each child’s events before merging. `EventActions` also carries a serialized `transfer_to_agent` field (exposed as `transferToAgent` in the [server](/docs/server) wire format) so external clients can observe hand-offs; in-process, the transfer signal travels through the `ToolContext` as described above.',
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'When the route is fixed, skip delegation entirely: [workflow agents](/docs/workflow-agents) compose the same specialists deterministically, with no extra model call spent on routing.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Agents overview](/docs/agents-overview) — the `BaseAgent` tree and `find_agent`.',
        '[Workflow agents](/docs/workflow-agents) — deterministic alternatives to LLM routing.',
        '[Built-in tools](/docs/builtin-tools) — `transfer_to_agent_tool`, `exit_loop`, and friends.',
        '[A2A](/docs/a2a) — delegating to agents running in other processes.',
      ],
    },
  ],
};
