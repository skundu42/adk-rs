# Agent Development Kit (adk-rs)

[![crates.io](https://img.shields.io/crates/v/adk-rs.svg)](https://crates.io/crates/adk-rs)
[![docs.rs](https://img.shields.io/docsrs/adk-rs)](https://docs.rs/adk-rs)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

An open-source, code-first Rust framework for building, evaluating, and deploying AI agents. `adk-rs` is a Rust port of Google's ADK: model-agnostic, deployment-agnostic, and wire-compatible with the Python ADK ecosystem (eval sets, `adk api_server` endpoints, the adk-web UI, and the A2A protocol) — with the low overhead, predictable latency, and safety guarantees of the Rust toolchain.

**📖 Full documentation: [adk-rs.vercel.app](https://adk-rs.vercel.app)**

## Highlights

- **Three providers, one trait** — Gemini, Anthropic Claude, and any OpenAI-compatible endpoint (Azure, Ollama, Groq) behind `Arc<dyn Model>`, with native SSE streaming, image input, automatic retry/backoff, and prompt caching on every provider.
- **Composable agents** — `LlmAgent`, `SequentialAgent`, `ParallelAgent`, and `LoopAgent` nest through one `BaseAgent` trait, driven by a unified event stream.
- **Ergonomic tools** — `#[tool]` on any async function derives the schema and wiring; or mount tools from any **MCP** server or **OpenAPI 3.x** spec.
- **Production runtime** — structured output, human-in-the-loop tool confirmation, pause/cancel/resume, context caching, event compaction, and semantic memory.
- **Serve and interoperate** — an axum dev server compatible with the adk-web UI, an embeddable CLI, spec-compliant **A2A** agent-to-agent JSON-RPC, and **Gemini Live** bidirectional audio streaming.
- **Secure by default** — HTTPS-or-loopback for credentials, loopback-only unauthenticated servers, sandboxed code execution, zero `unsafe`.

## Installation

Default features are empty — opt in to what you need:

```toml
[dependencies]
adk-rs = { version = "0.5", features = ["gemini", "macros"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"
```

Requires Rust **1.85+** (edition 2024). Each provider reads its key from the environment: `GOOGLE_API_KEY`, `ANTHROPIC_API_KEY`, or `OPENAI_API_KEY` (plus optional `OPENAI_BASE_URL`). See [Installation](https://adk-rs.vercel.app/docs/installation) for the full feature-flag reference.

## Quick start

```rust
use adk_rs::agents::LlmAgent;
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let agent = LlmAgent::builder("greeter")
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

## Defining a tool

Annotate any async function — the macro derives the JSON schema and the `Tool` impl:

```rust
use adk_rs::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
struct GetWeatherArgs {
    /// City name in English (e.g. "Paris").
    city: String,
}

#[derive(Serialize)]
struct WeatherReport {
    temp_c: f32,
    description: String,
}

/// Look up the current weather in `args.city`.
#[tool]
async fn get_weather(
    args: GetWeatherArgs,
    _ctx: &mut adk_rs::core::ToolContext,
) -> adk_rs::Result<WeatherReport> {
    Ok(WeatherReport { temp_c: 22.0, description: "sunny".into() })
}
```

Attach it with `.tool(get_weather())` on the agent builder.

## Going further

Everything below is covered in depth at **[adk-rs.vercel.app](https://adk-rs.vercel.app)**:

| Topic | Docs |
|---|---|
| Multi-agent composition & workflow pipelines | [Workflow agents](https://adk-rs.vercel.app/docs/workflow-agents) · [Multi-agent](https://adk-rs.vercel.app/docs/multi-agent) |
| Sessions, state, artifacts & semantic memory | [Sessions & state](https://adk-rs.vercel.app/docs/sessions-and-state) · [Memory](https://adk-rs.vercel.app/docs/memory) |
| MCP toolsets & OpenAPI tool generation | [MCP](https://adk-rs.vercel.app/docs/mcp) · [OpenAPI tools](https://adk-rs.vercel.app/docs/openapi-tools) |
| Structured output (schema-enforced on all providers) | [Structured output](https://adk-rs.vercel.app/docs/structured-output) |
| Human-in-the-loop confirmation, pause & resume | [Tool confirmation](https://adk-rs.vercel.app/docs/tool-confirmation) · [Cancellation & resume](https://adk-rs.vercel.app/docs/cancellation-and-resume) |
| Cost levers: context caching & event compaction | [Context caching](https://adk-rs.vercel.app/docs/context-caching) · [Event compaction](https://adk-rs.vercel.app/docs/event-compaction) |
| Authenticated tools (OAuth2, service accounts) | [Auth](https://adk-rs.vercel.app/docs/auth) |
| Sandboxed code execution (local / Docker) | [Code execution](https://adk-rs.vercel.app/docs/code-execution) |
| HTTP server (adk api_server-compatible) & CLI | [Server](https://adk-rs.vercel.app/docs/server) · [CLI](https://adk-rs.vercel.app/docs/cli) |
| A2A protocol & Gemini Live streaming | [A2A](https://adk-rs.vercel.app/docs/a2a) · [Gemini Live](https://adk-rs.vercel.app/docs/live) |
| Evaluation, testing & telemetry | [Eval](https://adk-rs.vercel.app/docs/eval) · [Testing](https://adk-rs.vercel.app/docs/testing) · [Telemetry](https://adk-rs.vercel.app/docs/telemetry) |
| Security model | [Security](https://adk-rs.vercel.app/docs/security) |

## Examples

Runnable demos live under [`examples/`](examples/):

```sh
cargo run --example gemini_chat --features gemini
cargo run --example weather_agent --features "gemini,macros"
cargo run --example three_providers --features "gemini,anthropic,openai"
cargo run --example code_agent --features "code-exec,testing"
# live API wire-compatibility smoke test (needs real keys):
cargo run --example compat_check --features "anthropic,openai"
```

The documentation site itself lives under [`docs/`](docs/) (`cd docs && npm install && npm run dev`); it is excluded from the published crate.

## Contributing

Bug reports, feature requests, and pull requests are welcome. Before submitting:

```sh
cargo fmt --all
cargo clippy --all-features --all-targets
cargo test --all-features
```

Please open an issue before starting on substantial changes.

## License

Licensed under the Apache License, Version 2.0 — see [LICENSE](LICENSE).
