import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'runner',
  title: 'The Runner',
  description:
    'How the Runner orchestrates invocations, persists events, and exposes run, start, resume, and cancel entry points.',
  srcPath: 'src/runner/runner.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'The Runner is the top-level orchestrator of every adk-rs program. It owns the agent tree and the four pluggable services, turns each user message into an invocation, persists the resulting events into the session, and hands you the event stream.',
    },
    {
      kind: 'p',
      text: 'A `Runner` holds one root [`BaseAgent`](/docs/agents-overview) plus the service quartet: a required [`SessionService`](/docs/sessions-and-state) and optional [`ArtifactService`](/docs/artifacts), [`MemoryService`](/docs/memory), and `CredentialService`. It also carries a `PluginManager` (see [Callbacks & plugins](/docs/callbacks-and-plugins)), app-level [context-cache](/docs/context-caching), [compaction](/docs/event-compaction), and [resumability](/docs/cancellation-and-resume) configuration, and a registry of in-flight invocations keyed by `invocation_id`.',
    },
    { kind: 'h2', text: 'Building a Runner' },
    {
      kind: 'p',
      text: '`Runner::builder()` returns a `RunnerBuilder`. Three settings are required — `app_name`, `agent`, and `session_service` — and `build()` returns a config error if any is missing.',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'fn app_name(self, name: impl Into<String>) -> Self', desc: 'App name (required). Scopes sessions, artifacts, and memory.' },
        { sig: 'fn agent(self, agent: Arc<dyn BaseAgent>) -> Self', desc: 'Root agent (required). Workflow agents nest their children under it.' },
        { sig: 'fn session_service(self, s: Arc<dyn SessionService>) -> Self', desc: 'Session backend (required). See [Sessions & state](/docs/sessions-and-state).' },
        { sig: 'fn artifact_service(self, s: Arc<dyn ArtifactService>) -> Self', desc: 'Optional artifact backend. See [Artifacts](/docs/artifacts).' },
        { sig: 'fn memory_service(self, s: Arc<dyn MemoryService>) -> Self', desc: 'Optional long-term memory backend. See [Memory](/docs/memory).' },
        { sig: 'fn credential_service(self, s: Arc<dyn CredentialService>) -> Self', desc: 'Optional credential store for authenticated tools. See [Auth](/docs/auth).' },
        { sig: 'fn auto_create_session(self, yes: bool) -> Self', desc: 'When a `session_id` is passed but not found, create it instead of returning a not-found error.' },
        { sig: 'fn context_cache_config(self, cfg: ContextCacheConfig) -> Self', desc: 'App-level explicit [context caching](/docs/context-caching). Copied into each invocation’s `RunConfig` unless the caller set one there.' },
        { sig: 'fn compaction(self, cfg: EventsCompactionConfig) -> Self', desc: 'Enable automatic [event compaction](/docs/event-compaction) after invocations complete. Best-effort: failures are logged, never surfaced.' },
        { sig: 'fn resumable(self, yes: bool) -> Self', desc: 'Sets `ResumabilityConfig { is_resumable }`: workflow agents record checkpoints and `Runner::resume` continues from the last one. See [Cancellation & resume](/docs/cancellation-and-resume).' },
        { sig: 'async fn plugin(self, p: Arc<dyn BasePlugin>) -> Result<Self>', desc: 'Register a plugin. Async (registration may do I/O) and fallible, so it returns `Result<Self>`.' },
        { sig: 'fn build(self) -> Result<Runner>', desc: 'Validate the required fields and construct the Runner.' },
      ],
    },
    { kind: 'h2', text: 'run, run_with, start, resume, cancel' },
    {
      kind: 'p',
      text: 'There are four ways into an invocation, each a thin layer over the previous. All of them load (or auto-create) the session, persist the user event first, then launch the root agent and forward its events — persisting every non-partial, non-user event through `SessionService::append_event_locked` before yielding it to you.',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'async fn run(&self, user_id, session_id: Option<&str>, user_text: &str) -> Result<EventStream<\'static>>', desc: 'One text turn with `RunConfig::default()`. The simplest entry point.' },
        { sig: 'async fn run_with(&self, user_id, session_id, user_content: Content, run_config: RunConfig) -> Result<EventStream<\'static>>', desc: 'Typed `Content` plus an explicit `RunConfig`. Convenience wrapper around `start` that returns just the stream.' },
        { sig: 'async fn start(&self, user_id, session_id, user_content: Content, run_config: RunConfig) -> Result<RunningInvocation>', desc: 'Start an invocation and return a handle carrying the generated `invocation_id`, the shared `CancellationToken`, and the event stream. Prefer this when you need to cancel or resume.' },
        { sig: 'async fn resume(&self, user_id, session_id: &str, invocation_id: &str, new_content: Option<Content>, run_config) -> Result<RunningInvocation>', desc: 'Resume a paused invocation **in place** — the run keeps its `invocation_id`. `new_content` typically carries the `FunctionResponse` that unblocks the pause: a long-running tool result, an `adk_request_confirmation` decision, or an `adk_request_credential` consent.' },
        { sig: 'fn cancel(&self, invocation_id: &str) -> bool', desc: 'Flip the cancellation token of an in-flight invocation. Returns `false` for unknown ids. Cooperative: agents check the flag between LLM calls and tool dispatches; in-flight tools run to completion.' },
        { sig: 'fn is_active(&self, invocation_id: &str) -> bool', desc: 'Whether an invocation with this id is currently registered as in-flight.' },
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: "A `RunningInvocation` has three public fields: `invocation_id: String`, `cancellation: CancellationToken`, and `events: EventStream<'static>`. The entry in the runner's active map is dropped automatically when the stream ends — by completion, error, cancellation, or caller-side drop.",
    },
    { kind: 'h2', text: 'RunConfig' },
    {
      kind: 'p',
      text: '`RunConfig` (in `adk_rs::core::run_config`) carries per-invocation overrides. `context_cache_config` and `resumability` are usually inherited from the runner; an explicit value on the `RunConfig` wins.',
    },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Meaning'],
      rows: [
        ['`streaming_mode`', '`StreamingMode`', '`None` (default) or `Sse`. See [Streaming](/docs/streaming).'],
        ['`max_llm_calls`', '`Option<u32>`', 'Stop after this many LLM turns. `None` = unlimited (use with care). Enforced by `InvocationContext::check_and_inc_llm_call`.'],
        ['`context_cache_config`', '`Option<ContextCacheConfig>`', 'Explicit context caching; overrides the runner-level setting.'],
        ['`resumability`', '`Option<ResumabilityConfig>`', 'Pause/resume behaviour; overrides the runner-level setting.'],
      ],
    },
    { kind: 'h2', text: 'InvocationContext' },
    {
      kind: 'p',
      text: 'For every invocation the runner builds an `Arc<InvocationContext>` and threads it through the agent tree, tools, callbacks, and plugins. It exposes the identity triple (`app_name`, `user_id`, `invocation_id`), the live session as `Arc<Mutex<Session>>` (so the runner and tool callbacks can both mutate state safely), all four services, the `run_config`, the original `user_content`, an LLM call counter, the shared `cancellation` token, an `attributes` bag (`Arc<Mutex<HashMap<String, Value>>>`) used by plugins and the auth/confirmation preprocessors, and `root_agent: Option<Arc<dyn BaseAgent>>` — set by the runner to the root of the agent tree and used by [agent transfer](/docs/multi-agent) to reach siblings and ancestors.',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'fn new_id() -> String', desc: 'Generate a fresh `inv-<uuid>` invocation id.' },
        { sig: 'fn check_and_inc_llm_call(&self) -> Result<()>', desc: 'Increment the LLM call counter; errors once `RunConfig::max_llm_calls` is reached.' },
        { sig: 'fn is_cancelled(&self) -> bool', desc: 'True if the cancellation token has been flipped. Agents call this at safe points to halt cleanly.' },
      ],
    },
    {
      kind: 'p',
      text: 'The `origin: InvocationOrigin` field records where the invocation came from — `Api` (default), `Cli`, or `Web`. It is mostly informational.',
    },
    { kind: 'h2', text: 'A complete example' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Runner with services, cancellation, and an explicit RunConfig',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::core::{RunConfig, StreamingMode};
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::Runner;
use adk_rs::services::mem::{
    InMemoryArtifactService, InMemoryMemoryService, InMemorySessionService,
};
use adk_rs::genai_types::Content;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let agent = LlmAgent::builder("assistant")
        .model(Arc::new(Gemini::from_env("gemini-2.5-flash")?))
        .instruction("You are a helpful assistant.")
        .build()?;

    let runner = Runner::builder()
        .app_name("demo")
        .agent(Arc::new(agent))
        .session_service(Arc::new(InMemorySessionService::new()))
        .artifact_service(Arc::new(InMemoryArtifactService::new()))
        .memory_service(Arc::new(InMemoryMemoryService::new()))
        .auto_create_session(true)
        .resumable(true)
        .build()?;

    // start() hands back the invocation id so you can cancel later.
    let run_config = RunConfig {
        streaming_mode: StreamingMode::Sse,
        max_llm_calls: Some(10),
        ..RunConfig::default()
    };
    let handle = runner
        .start("alice", Some("s1"), Content::user_text("Hello!"), run_config)
        .await?;
    println!("invocation: {}", handle.invocation_id);

    let mut events = handle.events;
    while let Some(event) = events.next().await {
        let event = event?;
        if let Some(content) = &event.response.content {
            println!("[{}] {}", event.author, content.text_concat());
        }
    }
    Ok(())
}`,
    },
    {
      kind: 'p',
      text: 'Because the example requests `StreamingMode::Sse`, the stream contains transient `partial: Some(true)` token events before the final aggregated event of each model turn. See [Streaming](/docs/streaming).',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Events](/docs/events) — the values the runner persists and yields.',
        '[Streaming](/docs/streaming) — `StreamingMode` and partial events.',
        '[Cancellation & resume](/docs/cancellation-and-resume) — pause/resume mechanics in depth.',
        '[Callbacks & plugins](/docs/callbacks-and-plugins) — `before_run`, `on_event`, `after_run` hooks.',
      ],
    },
  ],
};
