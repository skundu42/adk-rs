import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'callbacks-and-plugins',
  title: 'Callbacks & plugins',
  description:
    'Lifecycle hooks in adk-rs: the callback type aliases and their contexts, and the plugin system the Runner invokes around every invocation and event.',
  srcPath: 'src/core/callback.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs has two hook surfaces. **Callbacks** are typed async closures aimed at specific lifecycle points — before/after the agent, the model, and each tool. **Plugins** are trait objects the [Runner](/docs/runner) calls around every invocation and for every event. In v0.3.0, plugins are the fully wired runtime surface.',
    },
    { kind: 'h2', text: 'Callback contexts' },
    {
      kind: 'p',
      text: 'Two lightweight wrappers around `Arc<InvocationContext>` accompany the callback types (both in `src/core/callback.rs`):',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub struct ReadonlyContext { pub invocation: Arc<InvocationContext> }',
          desc: 'Read-only view of the invocation. This is what dynamic [instruction providers](/docs/llm-agent) receive — they can read session state, services and run config but are not expected to mutate.',
        },
        {
          sig: 'pub struct CallbackContext { pub invocation: Arc<InvocationContext> }',
          desc: 'Mutable callback context. Same shape today; the docs in source note a future revision may add per-callback mutation helpers.',
        },
      ],
    },
    { kind: 'h2', text: 'Callback types' },
    {
      kind: 'p',
      text: 'Each callback is a type alias for an `Arc<dyn Fn(...) -> BoxFuture<Result<Option<T>>>>` — cheap to clone and store. The `Option` return is the control signal: `None` means “continue normally”, `Some(value)` short-circuits or rewrites, depending on the hook.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'type BeforeAgentCallback = Arc<dyn Fn(&mut CallbackContext) -> BoxFuture<Result<Option<Content>>> + Send + Sync>',
          desc: 'Runs before an agent. Returning `Some(content)` short-circuits the agent entirely and uses that content as its sole response.',
        },
        {
          sig: 'type AfterAgentCallback = BeforeAgentCallback',
          desc: 'Runs after an agent. Returning `Some(content)` replaces the agent’s emitted content.',
        },
        {
          sig: 'type BeforeModelCallback = Arc<dyn Fn(&mut CallbackContext, &mut LlmRequest) -> BoxFuture<Result<Option<LlmResponse>>> + Send + Sync>',
          desc: 'Runs before each model call. May mutate the outgoing `LlmRequest` in place (system text, contents, config) or short-circuit the call by returning a synthetic `LlmResponse`.',
        },
        {
          sig: 'type AfterModelCallback = Arc<dyn Fn(&mut CallbackContext, &mut LlmResponse) -> BoxFuture<Result<Option<LlmResponse>>> + Send + Sync>',
          desc: 'Runs after each model call; may edit the response in place or return a replacement.',
        },
        {
          sig: 'type OnModelErrorCallback = Arc<dyn Fn(&mut CallbackContext, &mut LlmRequest, &Error) -> BoxFuture<Result<Option<LlmResponse>>> + Send + Sync>',
          desc: 'Recovery hook for model failures: inspect the error and request, optionally return a substitute `LlmResponse` instead of propagating the error.',
        },
        {
          sig: 'type BeforeToolCallback = Arc<dyn Fn(&mut ToolContext, &Arc<dyn DynTool>, &mut Value) -> BoxFuture<Result<Option<Value>>> + Send + Sync>',
          desc: 'Runs before a tool executes, with mutable access to the JSON args. Returning `Some(result)` skips the tool and uses that value as its response.',
        },
        {
          sig: 'type AfterToolCallback = Arc<dyn Fn(&mut ToolContext, &Arc<dyn DynTool>, &Value, &mut Value) -> BoxFuture<Result<Option<Value>>> + Send + Sync>',
          desc: 'Runs after a tool, seeing the original args and the mutable result; may rewrite the result in place or return a replacement.',
        },
        {
          sig: 'type OnToolErrorCallback = Arc<dyn Fn(&mut ToolContext, &Arc<dyn DynTool>, &Value, &Error) -> BoxFuture<Result<Option<Value>>> + Send + Sync>',
          desc: 'Recovery hook for tool failures: optionally return a substitute result value.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'The crate also exports a `before_agent_callback!` macro that turns an ordinary async closure into a `BeforeAgentCallback`-shaped value, sparing you the `Arc`/`BoxFuture` boilerplate.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Constructing callbacks',
      code: `use std::sync::Arc;
use adk_rs::before_agent_callback;
use adk_rs::core::{BeforeAgentCallback, BeforeModelCallback, CallbackContext};

// Via the macro: an async closure over &mut CallbackContext.
let gate: BeforeAgentCallback = before_agent_callback!(|_ctx| async move {
    // Return Some(content) to short-circuit the agent.
    Ok(None)
});

// By hand: a before-model callback that rewrites the outgoing request.
let guard: BeforeModelCallback = Arc::new(|_ctx: &mut CallbackContext, req| {
    Box::pin(async move {
        req.append_system_text("Never reveal internal tool names.");
        Ok(None) // None = proceed with the (mutated) request
    })
});`,
    },
    {
      kind: 'callout',
      tone: 'warn',
      title: 'Wiring status in v0.3.0',
      text: 'The callback aliases and macro are exported from `adk_rs::core`, but `LlmAgentBuilder` does not yet expose registration methods for them — there is no `.before_model_callback(...)` on the builder. For runtime interception today, use **plugins** (below); for per-turn prompt shaping, use a dynamic instruction provider.',
    },
    { kind: 'h2', text: 'The BasePlugin trait' },
    {
      kind: 'p',
      text: 'Plugins live in `src/runner/plugin.rs`. All hooks have safe defaults, so an implementation overrides only what it needs:',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'async fn on_register(&self) -> Result<()>',
          desc: 'Called once when the plugin is registered with the manager.',
        },
        {
          sig: 'async fn before_run(&self, ctx: &InvocationContext) -> Result<()>',
          desc: 'Called before each invocation begins (after the user event is persisted).',
        },
        {
          sig: 'async fn on_event(&self, ctx: &InvocationContext, event: &Event) -> Result<()>',
          desc: 'Called for every event the runner yields — model turns, tool responses, checkpoints.',
        },
        {
          sig: 'async fn after_run(&self, ctx: &InvocationContext, err: Option<&Error>) -> Result<()>',
          desc: 'Called when the invocation finishes, gracefully or with an error (the error is passed in).',
        },
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'Plugin hooks are fallible on purpose: an `Err` from `before_run` or `on_event` **aborts the invocation** — the runner surfaces the error to the caller and still invokes `after_run` with it. This makes plugins suitable for policy enforcement, not just observation.',
    },
    { kind: 'h2', text: 'PluginManager and registration' },
    {
      kind: 'p',
      text: '`PluginManager` holds an ordered `Vec<Arc<dyn BasePlugin>>` and fans every hook out to each plugin in registration order. You rarely touch it directly — `Runner::builder().plugin(...)` registers into the builder’s manager for you (note that `plugin` is async because it awaits `on_register`).',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub fn new() -> PluginManager',
          desc: 'Constructs an empty manager.',
        },
        {
          sig: 'pub async fn register(&mut self, p: Arc<dyn BasePlugin>) -> Result<()>',
          desc: 'Calls the plugin’s `on_register` and adds it to the fan-out list.',
        },
        {
          sig: 'pub async fn plugin(self, p: Arc<dyn BasePlugin>) -> Result<RunnerBuilder>',
          desc: 'On `RunnerBuilder`: registers a plugin with the runner being built.',
        },
      ],
    },
    { kind: 'h2', text: 'LoggingPlugin and a custom example' },
    {
      kind: 'p',
      text: 'The crate ships one plugin out of the box: `LoggingPlugin`, which logs every event at `INFO` under the `adk::event` target via the `tracing` facade (author, invocation id, concatenated text). It pairs well with the [telemetry](/docs/telemetry) feature.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Registering LoggingPlugin plus a custom counter',
      code: `use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use adk_rs::Runner;
use adk_rs::core::{Event, InvocationContext};
use adk_rs::error::Result;
use adk_rs::runner::{BasePlugin, LoggingPlugin};
use async_trait::async_trait;

#[derive(Debug, Default)]
struct EventCounter {
    seen: AtomicUsize,
}

#[async_trait]
impl BasePlugin for EventCounter {
    async fn on_event(&self, _ctx: &InvocationContext, _event: &Event) -> Result<()> {
        self.seen.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

let runner = Runner::builder()
    .app_name("hello")
    .agent(agent)
    .session_service(sessions)
    .plugin(Arc::new(LoggingPlugin)).await?
    .plugin(Arc::new(EventCounter::default())).await?
    .build()?;`,
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Runner](/docs/runner) — where plugin hooks fire in the invocation lifecycle.',
        '[Events](/docs/events) — the payload `on_event` observes.',
        '[LlmAgent](/docs/llm-agent) — dynamic instruction providers, the per-turn shaping hook that *is* wired today.',
        '[Telemetry](/docs/telemetry) — structured tracing to complement `LoggingPlugin`.',
      ],
    },
  ],
};
