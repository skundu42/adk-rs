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
      text: 'A coordinator `LlmAgent` registers specialists with `.sub_agent(...)` and exposes the `transfer_to_agent` built-in tool (`adk_rs::tools::transfer_to_agent_tool()`). The model sees a function declaration with one required parameter, `agent_name`, plus the description *“Transfer control to another agent by name…”* — and calls it when a request is better handled elsewhere.',
    },
    {
      kind: 'p',
      text: 'The flow, step by step: the model emits a `FunctionCall` for `transfer_to_agent`; the tool sets `transfer_to_agent` on its `ToolContext`; after the tool-response event is yielded, the `LlmAgent` resolves the name with `find_agent` (a depth-first search over its sub-agent tree), runs the target with the **same** `InvocationContext`, and streams the target’s events as the remainder of the invocation. The coordinator does not regain control that turn.',
    },
    {
      kind: 'list',
      items: [
        'The target shares the session, so it sees the full conversation including the user request and the transfer call.',
        'An unknown `agent_name` fails the run with a not-found error — names must be unique and stable within the tree.',
        '`.disable_transfer(true)` on the builder makes the agent ignore transfer requests: the tool still responds, but control never moves.',
      ],
    },
    {
      kind: 'callout',
      tone: 'warn',
      title: 'Register the tool explicitly',
      text: 'In v0.3.0 the transfer tool is **not** auto-injected when sub-agents are present. Add `.tool(transfer_to_agent_tool())` to the coordinator, and name the available specialists (with what they do) in its instruction so the model knows its options.',
    },
    { kind: 'h2', text: 'Coordinator / specialist example' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'A router with two specialists',
      code: `use std::sync::Arc;
use adk_rs::agents::LlmAgent;
use adk_rs::providers::gemini::Gemini;
use adk_rs::tools::transfer_to_agent_tool;

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

let coordinator = LlmAgent::builder("coordinator")
    .model(model)
    .description("Routes each request to the right specialist.")
    .instruction(
        "Decide who should handle the request and call transfer_to_agent. \\
         Available agents: greeter (greetings), task_executor (everything else).",
    )
    .tool(transfer_to_agent_tool())
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
