import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'introduction',
  title: 'Introduction',
  description:
    'What adk-rs is, the ideas behind it, and how the crate is organised.',
  srcPath: 'src/lib.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs is an open-source, code-first Rust framework for building, evaluating, and deploying AI agents — built for teams that want low overhead, predictable latency, and the safety guarantees of the Rust toolchain.',
    },
    {
      kind: 'p',
      text: 'ADK applies software-engineering discipline to agent construction: agents are plain values you compose in code, models are swappable behind one trait, and every run is an append-only stream of typed events. The crate is **model-agnostic** (Gemini, Anthropic, any OpenAI-compatible endpoint) and **deployment-agnostic** (a library first; the HTTP server, CLI, and A2A bridge are opt-in features).',
    },
    { kind: 'h2', text: 'Design at a glance' },
    {
      kind: 'list',
      items: [
        '**One crate, twenty feature flags.** `adk-rs` was originally a workspace of 17 sub-crates and is now a single feature-gated front door. Heavy dependencies (`sqlx`, `axum`, `reqwest`, OpenTelemetry) only compile when you opt in.',
        '**Agents are a trait.** [`BaseAgent`](/docs/agents-overview) is implemented by `LlmAgent` and by the `Sequential`/`Parallel`/`Loop` workflow agents — and by anything you write yourself. Composition is just nesting values.',
        '**Everything is an event.** A run produces an `EventStream` — a `Stream` of [`Event`](/docs/events) values carrying content, tool calls, state deltas, and control actions. Sessions are append-only event logs.',
        '**Services are pluggable.** Session, artifact, memory, and credential storage are traits with in-memory, filesystem, SQLite, and PostgreSQL implementations.',
        '**Secure by default.** Credential-bearing clients refuse plaintext HTTP, dev servers refuse non-loopback binds without auth, and the Docker code executor is locked down out of the box. See [Security](/docs/security).',
      ],
    },
    { kind: 'h2', text: 'A complete agent in ~25 lines' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/main.rs',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let agent = LlmAgent::builder("greeter")
        .description("A friendly greeter")
        .model(Arc::new(Gemini::from_env("gemini-2.5-flash")?))
        .instruction("You greet the user warmly.")
        .build()?;

    let runner = Runner::builder()
        .app_name("hello")
        .agent(Arc::new(agent))
        .session_service(Arc::new(InMemorySessionService::new()))
        .build()?;

    let mut events = runner.run("user", None, "Hello!").await?;
    while let Some(event) = events.next().await {
        if let Some(content) = event?.response.content {
            println!("{}", content.text_concat());
        }
    }
    Ok(())
}`,
    },
    {
      kind: 'p',
      text: 'Three pieces appear in every adk-rs program: an **agent** (here an [`LlmAgent`](/docs/llm-agent) talking to Gemini), a [**runner**](/docs/runner) that owns services and orchestrates invocations, and the **event stream** you consume with ordinary `futures` combinators.',
    },
    { kind: 'h2', text: 'Crate layout' },
    {
      kind: 'table',
      head: ['Module', 'Feature gate', 'Responsibility'],
      rows: [
        ['`agents`', 'always on', '`BaseAgent`, `LlmAgent`, `SequentialAgent`, `ParallelAgent`, `LoopAgent`.'],
        ['`core`', 'always on', 'Domain primitives: `Event`, `Session`, `State`, `LlmRequest`/`LlmResponse`, contexts, service traits.'],
        ['`genai_types`', 'always on', 'Wire-neutral data: `Content`, `Part`, `Schema`, `FunctionCall`, `GenerateContentConfig`.'],
        ['`tools`', 'always on', '`Tool` trait, `FunctionTool`, built-ins, toolsets.'],
        ['`runner`', 'always on', 'Orchestration: `Runner`, plugins, event compaction.'],
        ['`services`', 'mem always; `fs`, `sqlite`, `postgres`', 'Session / artifact / memory / credential backends.'],
        ['`providers`', '`gemini` / `anthropic` / `openai`', 'LLM provider clients.'],
        ['`auth`', 'types always; flows behind `auth`', 'OAuth2, service accounts, API keys, consent flows.'],
        ['`mcp`', '`mcp`', 'Model Context Protocol toolset (stdio + streamable HTTP).'],
        ['`a2a`', '`a2a`', 'Agent-to-Agent JSON-RPC client + server bridge.'],
        ['`code_exec`', '`code-exec` (+ `code-exec-docker`)', 'Local and Docker code executors.'],
        ['`server`', '`server`', 'axum dev server, ADK web-UI compatible endpoints.'],
        ['`eval`', '`eval`', 'Eval-set replay and scoring.'],
        ['`telemetry`', '`telemetry` (+ `otel`)', 'tracing setup, optional OTLP export.'],
        ['`cli`', '`cli`', 'Embeddable clap-based CLI.'],
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'The `#[tool]` proc-macro lives in a sibling crate, `adk-rs-macros`, because Rust requires proc-macros to be their own crate. Enable it through the `macros` feature — you never depend on the macros crate directly.',
    },
    { kind: 'h2', text: 'Where to go next' },
    {
      kind: 'list',
      items: [
        '[Installation](/docs/installation) — feature flags and provider credentials.',
        '[Quickstart](/docs/quickstart) — build and run your first agent.',
        '[Agents overview](/docs/agents-overview) — the `BaseAgent` trait and the four built-in agent kinds.',
        '[Examples](/docs/examples/gemini-chat) — annotated walkthroughs of every example in the repo.',
      ],
    },
  ],
};
