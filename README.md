# Agent Development Kit (RUST)

An open-source, code-first Rust framework for building, evaluating, and deploying sophisticated AI agents with flexibility and control.

Agent Development Kit (ADK) is a flexible, modular framework that applies software-engineering discipline to AI-agent construction. `adk-rs` is a Rust port of the Google's ADK Python implementation, aimed at teams that want low overhead, predictable latency, and the safety guarantees of the Rust toolchain. Like its Python counterpart, ADK is model-agnostic, deployment-agnostic, and integrates cleanly alongside other frameworks.

## ✨ Key Features

- **First-class providers** — Gemini (REST + SSE), Anthropic Claude (Messages API + SSE), and an OpenAI-compatible client that also serves Azure OpenAI, Ollama, and Groq via base-URL override.
- **Composable agent primitives** — `LlmAgent`, `SequentialAgent`, `ParallelAgent`, and `LoopAgent`, all driven by a unified event stream over `tokio`.
- **Ergonomic tools** — annotate any async function with `#[adk_rs_tools::tool]`; the macro derives the JSON schema, the `FunctionDeclaration`, and a `Tool` impl. Manual implementations remain available as an escape hatch.
- **Pluggable services** — session, memory, artifact, and credential traits with in-memory, filesystem, SQLite, and PostgreSQL backends out of the box.
- **MCP toolset** — connect to any Model Context Protocol server over stdio.
- **Production telemetry** — `tracing` integration with optional OpenTelemetry OTLP export.
- **Evaluation framework** — replay JSON eval sets (compatible with the Python ADK format) and score with trajectory and LLM-judge metrics.
- **Dev server + CLI scaffolding** — an `axum`-based HTTP/SSE server and a library-style CLI that you embed in your own binary.
- **Safety first** — `#![forbid(unsafe_code)]` across the workspace; pedantic Clippy enabled; no global mutable state.

## 🚀 Installation

`adk-rs` is published as a Cargo workspace. Most users depend on a handful of crates directly:

```toml
[dependencies]
adk-rs-agents = "0.1"
adk-rs-runner = "0.1"
adk-rs-providers-gemini = "0.1"
adk-rs-services-mem = "0.1"
adk-rs-tools = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"
```

Requires Rust **1.85+** (edition 2024).

### Provider credentials

Each provider reads its API key from the environment:

| Provider | Variable |
|---|---|
| Gemini | `GOOGLE_API_KEY` |
| Anthropic | `ANTHROPIC_API_KEY` |
| OpenAI-compatible | `OPENAI_API_KEY` (plus optional `OPENAI_BASE_URL`) |

## 🏁 Quick start

Define a single agent and stream its events to stdout:

```rust
use adk_rs_agents::LlmAgent;
use adk_rs_providers_gemini::Gemini;
use adk_rs_runner::Runner;
use adk_rs_services_mem::InMemorySessionService;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> adk_rs_error::Result<()> {
    let agent = LlmAgent::builder("greeter")
        .description("A friendly greeter")
        .model(Arc::new(Gemini::from_env("gemini-2.5-flash")?))
        .instruction("You greet the user warmly.")
        .build()?;

    let runner = Runner::builder()
        .app_name("hello")
        .agent(Arc::new(agent))
        .session_service(Arc::new(InMemorySessionService::default()))
        .build()?;

    let mut events = runner.run("user", None, "Hello!").await?;
    while let Some(event) = events.next().await {
        if let Some(content) = event?.response.content {
            println!("{}", content.text_concat());
        }
    }
    Ok(())
}
```

## 🤝 Multi-agent composition

Agents nest via the same `BaseAgent` trait. A coordinator can delegate to specialised children, with the runner choosing between them based on each agent's description:

```rust
use adk_rs_agents::LlmAgent;
use std::sync::Arc;

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
    .description("I route the request to the right specialist.")
    .sub_agent(greeter)
    .sub_agent(task_executor)
    .build()?;
```

`SequentialAgent`, `ParallelAgent`, and `LoopAgent` provide explicit orchestration when LLM-driven delegation is not appropriate.

## 🛠 Defining a tool

Add `#[tool]` to any async function. The macro derives a JSON schema from the arguments struct and returns a constructor for an `Arc<dyn Tool>` that can be handed to `LlmAgent::builder().tool(...)`.

```rust
use adk_rs_tools::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
struct GetWeatherArgs {
    /// City name in English (e.g. "Paris").
    city: String,
}

#[derive(Serialize)]
struct WeatherReport {
    city: String,
    temp_c: f32,
    description: String,
}

/// Look up the current weather in `args.city`.
#[tool]
async fn get_weather(
    args: GetWeatherArgs,
    _ctx: &mut adk_rs_core::ToolContext,
) -> adk_rs_error::Result<WeatherReport> {
    Ok(WeatherReport {
        city: args.city,
        temp_c: 22.0,
        description: "sunny".into(),
    })
}
```

Attach it with `.tool(get_weather())` on the agent builder.

## 💻 Embedding the CLI

Unlike the Python CLI, Rust agents are statically linked. Build your own binary on top of `adk_rs_cli::App`:

```rust
use std::sync::Arc;

fn main() -> adk_rs_error::Result<()> {
    adk_rs_cli::App::new("my-app")
        .register("greeter", Arc::new(build_greeter()?))
        .run()
}
```

This gives you four subcommands out of the box:

```sh
my-app run --agent greeter "Hello!"     # single-turn invocation
my-app web --bind 127.0.0.1:8000        # axum dev server with SSE
my-app eval --agent greeter --set hello.evalset.json
my-app version
```

## 🌐 Dev web server

`adk-rs-server` exposes the runner over HTTP and Server-Sent Events for local testing and integration with web frontends. The `web` subcommand above starts it; for embedding directly, see [`adk-rs-server`](adk-rs-server/).

## 📊 Evaluating agents

The eval framework loads eval-set JSON files compatible with the Python ADK format, replays them through any `BaseAgent`, and scores with trajectory and response-match metrics. From the CLI:

```sh
my-app eval --agent greeter --set samples/hello_world.evalset.json
```

Programmatically:

```rust
let bytes = tokio::fs::read("hello_world.evalset.json").await?;
let set: adk_rs_eval::EvalSet = serde_json::from_slice(&bytes)?;
let runner = adk_rs_eval::EvalRunner::new(
    agent,
    "hello_world".into(),
    "eval-user",
    vec![
        Arc::new(adk_rs_eval::TrajectoryMatch::new(1.0)),
        Arc::new(adk_rs_eval::ResponseMatch::new(0.5)),
    ],
);
let report = runner.run_set(&set).await?;
```

## 📦 Workspace layout

`adk-rs` is split into focused crates so consumers pull only what they need. Heavy dependencies (sqlx, axum, OTel) stay behind crate boundaries.

| Crate | Responsibility |
|---|---|
| [`adk-rs-error`](adk-rs-error/) | Workspace `Error` / `Result` and error codes. |
| [`adk-rs-genai-types`](adk-rs-genai-types/) | Wire-neutral data: `Content`, `Part`, `Schema`, `FunctionCall`, `GenerateContentConfig`. |
| [`adk-rs-core`](adk-rs-core/) | Domain primitives: `Event`, `Session`, `State`, `LlmRequest/Response`, `InvocationContext`, service traits. |
| [`adk-rs-services-mem`](adk-rs-services-mem/) | In-memory session, memory, artifact, and credential services. |
| [`adk-rs-services-fs`](adk-rs-services-fs/) | Filesystem artifact service. |
| [`adk-rs-services-sql`](adk-rs-services-sql/) | SQL `SessionService` over `sqlx` (SQLite + PostgreSQL). |
| [`adk-rs-providers-gemini`](adk-rs-providers-gemini/) | Gemini REST + SSE provider. |
| [`adk-rs-providers-anthropic`](adk-rs-providers-anthropic/) | Anthropic Messages API + SSE provider. |
| [`adk-rs-providers-openai`](adk-rs-providers-openai/) | OpenAI-compatible provider (Azure / Ollama / Groq via base-URL). |
| [`adk-rs-tools`](adk-rs-tools/) | `Tool` trait, `FunctionTool`, built-ins. |
| [`adk-rs-tools-macros`](adk-rs-tools-macros/) | `#[tool]` proc-macro. |
| [`adk-rs-agents`](adk-rs-agents/) | `BaseAgent` trait plus `LlmAgent`, `SequentialAgent`, `ParallelAgent`, `LoopAgent`. |
| [`adk-rs-runner`](adk-rs-runner/) | Orchestration: LLM flow, tool dispatch, agent transfer, plugin manager. |
| [`adk-rs-mcp`](adk-rs-mcp/) | MCP stdio client and `McpToolset`. |
| [`adk-rs-telemetry`](adk-rs-telemetry/) | `tracing-subscriber` setup with optional OTLP export. |
| [`adk-rs-eval`](adk-rs-eval/) | Eval-set IO and metrics. |
| [`adk-rs-server`](adk-rs-server/) | `axum` dev server with SSE. |
| [`adk-rs-cli`](adk-rs-cli/) | Embeddable CLI library and reference `adk` binary. |

## 🧩 Examples

Runnable demos live under [`examples/src/bin`](examples/src/bin):

- [`gemini_chat`](examples/src/bin/gemini_chat.rs) — minimal single-agent loop.
- [`weather_agent`](examples/src/bin/weather_agent.rs) — `#[tool]`-defined function tool.
- [`three_providers`](examples/src/bin/three_providers.rs) — the same prompt against Gemini, Claude, and OpenAI.

```sh
cargo run -p adk-rs-examples --bin weather_agent
```

## 🤝 Contributing

Bug reports, feature requests, and pull requests are welcome. Before submitting:

```sh
cargo fmt --all
cargo clippy --workspace --all-features --all-targets
cargo test --workspace --all-features
```

Please open an issue before starting on substantial changes.

## 📄 License

Licensed under the Apache License, Version 2.0 — see [LICENSE](LICENSE).
