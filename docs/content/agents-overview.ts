import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'agents-overview',
  title: 'Agents overview',
  description:
    'The BaseAgent trait that every agent implements, the four built-in agent kinds, and how agents observe the invocation context.',
  srcPath: 'src/agents/base.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Every agent in adk-rs — LLM-powered or hand-written — implements one trait: `BaseAgent`. An agent has a name, a description the model reads, optional children, and a single `run` method that turns an invocation into a stream of events.',
    },
    {
      kind: 'p',
      text: 'Because the contract is one async trait, composition is just nesting values: an [`LlmAgent`](/docs/llm-agent) holds `Arc<dyn BaseAgent>` children, a [`SequentialAgent`](/docs/workflow-agents) holds a `Vec` of them, and your own type can hold whatever it likes. The [runner](/docs/runner) only ever sees the root.',
    },
    { kind: 'h2', text: 'The BaseAgent trait' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/agents/base.rs',
      code: `#[async_trait]
pub trait BaseAgent: Send + Sync + std::fmt::Debug + 'static {
    fn name(&self) -> &str;

    fn description(&self) -> &str {
        ""
    }

    fn sub_agents(&self) -> &[Arc<dyn BaseAgent>] {
        &[]
    }

    fn find_agent(&self, name: &str) -> Option<Arc<dyn BaseAgent>> {
        // default impl: depth-first search over sub_agents()
    }

    async fn run(self: Arc<Self>, ctx: Arc<InvocationContext>) -> Result<EventStream<'static>>;
}`,
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn name(&self) -> &str',
          desc: 'The agent’s name. Must be a valid identifier and unique within an agent tree; the framework addresses agents by name for transfer.',
        },
        {
          sig: 'fn description(&self) -> &str',
          desc: 'Human-readable description, **shown to the model**. Keep it short and informative — an LLM parent uses it to decide whether to delegate. Defaults to the empty string.',
        },
        {
          sig: 'fn sub_agents(&self) -> &[Arc<dyn BaseAgent>]',
          desc: 'Direct children in the agent tree. Defaults to an empty slice.',
        },
        {
          sig: 'fn find_agent(&self, name: &str) -> Option<Arc<dyn BaseAgent>>',
          desc: 'Resolves a descendant by name, depth-first. The default implementation recurses through `sub_agents()`; `LlmAgent` uses it to route `transfer_to_agent` calls.',
        },
        {
          sig: 'async fn run(self: Arc<Self>, ctx: Arc<InvocationContext>) -> Result<EventStream<\'static>>',
          desc: 'Runs the agent within the given invocation context and returns a stream of events. The stream terminates when the agent has produced its final response (or errors out). Note the `Arc<Self>` receiver: agents are always run as shared values.',
        },
      ],
    },
    {
      kind: 'p',
      text: '`EventStream` is a type alias defined in `src/core/stream.rs`: `Pin<Box<dyn Stream<Item = Result<Event>> + Send + \'a>>` — an owned, dynamically-typed `futures::Stream` of [`Event`](/docs/events) values. You consume it with ordinary combinators (`next`, `collect`, `try_for_each`).',
    },
    { kind: 'h2', text: 'The four built-in agents' },
    {
      kind: 'table',
      head: ['Agent', 'Construction', 'Behaviour'],
      rows: [
        [
          '[`LlmAgent`](/docs/llm-agent)',
          '`LlmAgent::builder("name")...build()?`',
          'Talks to a model in an LLM↔tool loop; can hold tools, sub-agents, structured-output schemas, and a code executor.',
        ],
        [
          '[`SequentialAgent`](/docs/workflow-agents)',
          '`SequentialAgent::new(name, desc, subs)?`',
          'Runs sub-agents one after another; each child sees the cumulative event history.',
        ],
        [
          '[`ParallelAgent`](/docs/workflow-agents)',
          '`ParallelAgent::new(name, desc, subs)?`',
          'Spawns every child concurrently and merges their event streams, tagging each event with a branch label.',
        ],
        [
          '[`LoopAgent`](/docs/workflow-agents)',
          '`LoopAgent::new(name, desc, subs, max_iterations)?`',
          'Repeats its sub-agents until one escalates (e.g. via the `exit_loop` tool) or the iteration cap is hit.',
        ],
      ],
    },
    {
      kind: 'p',
      text: 'The deterministic workflow agents never call a model themselves — orchestration is plain Rust control flow. Use them when ordering matters; use `LlmAgent` sub-agents with [transfer](/docs/multi-agent) when the *model* should pick the route.',
    },
    { kind: 'h2', text: 'What agents observe: InvocationContext' },
    {
      kind: 'p',
      text: 'The `Arc<InvocationContext>` passed to `run` is the per-invocation world view, constructed by the runner. Every agent in the tree shares the same context (parallel branches included), so state, the LLM-call budget and cancellation propagate everywhere.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub session: Arc<Mutex<Session>>',
          desc: 'The live session being mutated. Agents push their events here; the runner mirrors them into the `SessionService`.',
        },
        {
          sig: 'pub user_content: Option<Content>',
          desc: 'The user content for this invocation, if any (resumed invocations may carry only a `FunctionResponse`).',
        },
        {
          sig: 'pub cancellation: CancellationToken',
          desc: 'Cooperative cancellation flag, flipped by `Runner::cancel` or the A2A `tasks/cancel` handler. See [Cancellation & resume](/docs/cancellation-and-resume).',
        },
        {
          sig: 'pub fn is_cancelled(&self) -> bool',
          desc: 'True once the token has flipped. Agents check this at safe points — between LLM↔tool iterations, between sub-agents — and halt cleanly.',
        },
        {
          sig: 'pub fn check_and_inc_llm_call(&self) -> Result<()>',
          desc: 'Increments the shared LLM-call counter; errors when `RunConfig::max_llm_calls` is reached. `LlmAgent` calls this before every model request.',
        },
        {
          sig: 'pub fn new_id() -> String',
          desc: 'Generates a fresh `inv-{uuid}` invocation id.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'The context also carries `app_name`, `user_id`, `invocation_id`, the optional artifact/memory/credential services, the per-invocation `RunConfig`, an `InvocationOrigin` (`Api`, `Cli`, or `Web`), and a free-form `attributes` bag that the framework uses for pause/resume bookkeeping.',
    },
    { kind: 'h2', text: 'Writing a custom agent' },
    {
      kind: 'p',
      text: 'Implementing `BaseAgent` by hand is the escape hatch for logic that is neither an LLM call nor a canned workflow. Build one or more `Event` values and return them as a pinned, boxed stream:',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'A hand-written echo agent',
      code: `use std::sync::Arc;

use adk_rs::Result;
use adk_rs::agents::BaseAgent;
use adk_rs::core::{Event, EventStream, InvocationContext};
use async_trait::async_trait;

#[derive(Debug)]
struct EchoAgent {
    name: String,
}

#[async_trait]
impl BaseAgent for EchoAgent {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "Echoes the user's message back."
    }

    async fn run(self: Arc<Self>, ctx: Arc<InvocationContext>) -> Result<EventStream<'static>> {
        let text = ctx
            .user_content
            .as_ref()
            .map(|c| c.text_concat())
            .unwrap_or_default();
        let mut event = Event::model_text(self.name.clone(), format!("echo: {text}"));
        event.invocation_id = ctx.invocation_id.clone();
        Ok(Box::pin(futures::stream::once(async move { Ok(event) })))
    }
}`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'A custom agent slots in anywhere a built-in does: pass `Arc::new(EchoAgent { .. })` to `Runner::builder().agent(...)`, nest it inside a `SequentialAgent`, or wrap it in an `AgentTool` so an `LlmAgent` can call it like a function.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[LlmAgent](/docs/llm-agent) — the builder, instruction templating, and the LLM↔tool loop.',
        '[Workflow agents](/docs/workflow-agents) — Sequential, Parallel and Loop in depth.',
        '[Multi-agent systems](/docs/multi-agent) — transfer, `AgentTool`, and coordinator patterns.',
        '[Events](/docs/events) — the `Event` payload your stream yields.',
        '[Runner](/docs/runner) — how invocations are created and persisted.',
      ],
    },
  ],
};
