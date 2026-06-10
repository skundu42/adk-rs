import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'workflow-agents',
  title: 'Workflow agents',
  description:
    'Deterministic orchestration with SequentialAgent, ParallelAgent and LoopAgent — ordering, branching and looping without a model in charge.',
  srcPath: 'src/agents/sequential_agent.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'The three workflow agents orchestrate other agents with plain Rust control flow. No model decides the route: a `SequentialAgent` runs children in order, a `ParallelAgent` fans out concurrently, and a `LoopAgent` repeats until a child escalates or an iteration cap is hit.',
    },
    {
      kind: 'p',
      text: 'All three implement [`BaseAgent`](/docs/agents-overview), so they nest freely — a sequence can contain a loop that contains a parallel fan-out, and any leaf can be an [`LlmAgent`](/docs/llm-agent) or a hand-written agent. Every child shares the parent’s `InvocationContext`, hence the same session, cancellation token and LLM-call budget.',
    },
    { kind: 'h2', text: 'SequentialAgent' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub fn new(name: impl Into<String>, description: impl Into<String>, sub_agents: Vec<Arc<dyn BaseAgent>>) -> Result<Self>',
          desc: 'Constructs the sequence. Errors if `sub_agents` is empty.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'Children run one after another, in declared order, and their event streams are forwarded as-is. Because each child pushes events into the shared session before the next child starts, **every child sees the cumulative history** — agent N reads everything agents 1..N-1 produced. The common pattern is pairing this with `output_key` on each child so later steps can reference earlier results as `{key}` placeholders.',
    },
    {
      kind: 'list',
      items: [
        '**Escalation stops the pipeline.** If a child yields an event with `actions.escalate == Some(true)`, the sequence forwards it and returns.',
        '**Pauses suspend the pipeline.** When a child pauses the invocation (a long-running tool, a [confirmation](/docs/tool-confirmation), or an auth consent), the sequence stops issuing further children.',
        '**Cancellation is checked between children** via `ctx.is_cancelled()`.',
      ],
    },
    { kind: 'h3', text: 'Resumability checkpoints' },
    {
      kind: 'p',
      text: 'With `Runner::builder().resumable(true)`, the sequence emits a checkpoint event after each completed child except the last — an event whose `actions.agent_state` records `{"completed_sub_agents": n}`. When the same invocation is later continued with `Runner::resume`, the sequence reads the latest checkpoint for its `invocation_id` and skips the children that already finished, re-entering exactly where it paused. See [Cancellation & resume](/docs/cancellation-and-resume).',
    },
    { kind: 'h2', text: 'ParallelAgent' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub fn new(name: impl Into<String>, description: impl Into<String>, sub_agents: Vec<Arc<dyn BaseAgent>>) -> Result<Self>',
          desc: 'Constructs the fan-out. Errors if `sub_agents` is empty.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'Each child runs on its own `tokio::spawn` task; events are funnelled through an mpsc channel and surfaced as one merged stream in arrival order. Before forwarding, the parent stamps each event’s `branch` field with `"{parent_name}.{index}"` (unless the child already set one), so consumers can attribute interleaved events to their originating branch.',
    },
    {
      kind: 'list',
      items: [
        'Child errors are forwarded into the merged stream as `Err` items; healthy siblings keep streaming.',
        'On cancellation the merged stream drops its receiver and returns; in-flight child tasks observe the same `CancellationToken` and exit themselves.',
        'All children share one session behind a mutex — concurrent state writes are safe but unordered, so give parallel children distinct `output_key`s.',
      ],
    },
    { kind: 'h2', text: 'LoopAgent' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub fn new(name: impl Into<String>, description: impl Into<String>, sub_agents: Vec<Arc<dyn BaseAgent>>, max_iterations: u32) -> Result<Self>',
          desc: 'Constructs the loop. Errors if `sub_agents` is empty or `max_iterations == 0`.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'Each iteration runs the sub-agents in order, exactly like a sequence; the whole cycle repeats up to `max_iterations` times. The loop breaks early when any forwarded event carries `actions.escalate == Some(true)`, when the invocation pauses, or on cancellation.',
    },
    {
      kind: 'p',
      text: 'The idiomatic way for an LLM child to break the loop is the **`exit_loop` built-in tool** (`adk_rs::tools::exit_loop()`): when the model calls it, the tool sets `escalate` on the `ToolContext`, the `LlmAgent` emits an escalation event, and the `LoopAgent` stops. See [Built-in tools](/docs/builtin-tools).',
    },
    { kind: 'h2', text: 'Composing all three' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Research in parallel, then draft and refine in a loop',
      code: `use std::sync::Arc;
use adk_rs::agents::{LlmAgent, LoopAgent, ParallelAgent, SequentialAgent};
use adk_rs::providers::gemini::Gemini;
use adk_rs::tools::exit_loop;

let model = Arc::new(Gemini::from_env("gemini-2.5-flash")?);

let research_web = Arc::new(
    LlmAgent::builder("research_web")
        .model(model.clone())
        .instruction("Summarize what the web says about the topic.")
        .output_key("web_notes")
        .build()?,
);
let research_docs = Arc::new(
    LlmAgent::builder("research_docs")
        .model(model.clone())
        .instruction("Summarize the internal docs on the topic.")
        .output_key("doc_notes")
        .build()?,
);
let research = Arc::new(ParallelAgent::new(
    "research",
    "Gathers notes from two sources concurrently.",
    vec![research_web, research_docs],
)?);

let drafter = Arc::new(
    LlmAgent::builder("drafter")
        .model(model.clone())
        .instruction("Write a draft using {web_notes} and {doc_notes}. \\
                      Revise it if a critique exists: {critique?}")
        .output_key("draft")
        .build()?,
);
let critic = Arc::new(
    LlmAgent::builder("critic")
        .model(model.clone())
        .instruction("Critique the draft: {draft}. \\
                      If it is good enough, call exit_loop.")
        .output_key("critique")
        .tool(exit_loop())
        .build()?,
);
let refine = Arc::new(LoopAgent::new(
    "refine",
    "Draft, critique, repeat until accepted.",
    vec![drafter, critic],
    5,
)?);

let pipeline = Arc::new(SequentialAgent::new(
    "writer_pipeline",
    "Research, then iteratively write.",
    vec![research, refine],
)?);`,
    },
    {
      kind: 'p',
      text: 'Pass `pipeline` to `Runner::builder().agent(...)` like any other agent. The parallel stage writes `web_notes` and `doc_notes` into state; the loop alternates drafter and critic until the critic calls `exit_loop` or five iterations elapse.',
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'Reach for workflow agents whenever the control flow is known in advance — they are cheaper, faster, and perfectly reproducible. Reserve [LLM-driven delegation](/docs/multi-agent) for routing decisions that genuinely require judgment.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Multi-agent systems](/docs/multi-agent) — model-chosen routing via transfer and `AgentTool`.',
        '[Sessions & state](/docs/sessions-and-state) — how `output_key` values travel between steps.',
        '[Cancellation & resume](/docs/cancellation-and-resume) — checkpoints and in-place resume.',
        '[Multi-agent pipeline guide](/docs/guides/multi-agent-pipeline) — a longer worked example.',
      ],
    },
  ],
};
