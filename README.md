# Agent Development Kit (adk-rs)

An open-source, code-first Rust framework for building, evaluating, and deploying sophisticated AI agents with flexibility and control.

This is a flexible, modular framework that applies software-engineering discipline to AI-agent construction. `adk-rs` is a Rust port of the Google's ADK Python implementation, aimed at teams that want low overhead, predictable latency, and the safety guarantees of the Rust toolchain. Like its Python counterpart, ADK is model-agnostic, deployment-agnostic, and integrates cleanly alongside other frameworks.

## ✨ Key Features

- **First-class providers** — Gemini (REST + SSE) with server-side built-ins (`google_search`, `url_context`, `built_in_code_execution`), Anthropic Claude (Messages API + SSE), and an OpenAI-compatible client that also serves Azure OpenAI, Ollama, and Groq via base-URL override.
- **Composable agent primitives** — `LlmAgent`, `SequentialAgent`, `ParallelAgent`, and `LoopAgent`, all driven by a unified event stream over `tokio` and a cooperative `CancellationToken`.
- **Ergonomic tools** — annotate any async function with `#[adk_rs::tool]`; the macro derives the JSON schema, the `FunctionDeclaration`, and a `Tool` impl. Manual implementations remain available as an escape hatch.
- **Pluggable services** — session, memory, artifact, and credential traits with in-memory, filesystem, SQLite, and PostgreSQL backends out of the box.
- **MCP toolset** — connect to any Model Context Protocol server over stdio *or* streamable HTTP (with `Mcp-Session-Id` echo and SSE-response support).
- **A2A protocol** — wire-compatible Google Agent-to-Agent JSON-RPC client + server bridge: `message/send`, `message/stream`, `tasks/get` / `cancel` / `resubscribe`, `tasks/pushNotificationConfig/*` webhook delivery, and `/.well-known/agent.json` discovery. Talk to Python `google-adk` agents and vice versa.
- **Authenticated tools** — full OAuth 2.0 (auth-code + PKCE, client-credentials, refresh), Service Account JWTs, API keys, and HTTP basic/bearer with interactive-consent suspend/resume.
- **OpenAPI generator** — point at a 3.x spec; get one tool per operation with security schemes mapped to `AuthConfig`.
- **Sandboxed code execution** — local subprocess or locked-down Docker container (cap-drop, no-new-privileges, memory / CPU / pids caps, non-root user, no network, read-only rootfs).
- **Production telemetry** — `tracing` integration with optional OpenTelemetry OTLP export.
- **Evaluation framework** — replay JSON eval sets (compatible with the Python ADK format) and score with trajectory and response-match metrics.
- **Dev server + CLI scaffolding** — an `axum`-based HTTP/SSE server (with bearer-token auth + loopback-default) and a library-style CLI that you embed in your own binary.
- **Secure by default** — refuses to send API keys / OAuth tokens over plaintext HTTP, refuses non-loopback binds without auth, and sanitises filesystem artifact paths against `..` traversal.

## 🚀 Installation

`adk-rs` ships as a single crate with cargo features. Opt in to the providers, storage backends, and subsystems you need:

```toml
[dependencies]
adk-rs = { version = "0.1", features = ["gemini", "macros"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"
```

### Available features

| Feature | Pulls in |
|---|---|
| `gemini` / `anthropic` / `openai` | the matching LLM provider |
| `fs` | filesystem artifact service |
| `sqlite` / `postgres` | SQL session backend |
| `mcp` | Model Context Protocol — stdio + streamable HTTP transports |
| `telemetry` | `tracing-subscriber` setup (add `otel` for OpenTelemetry OTLP export) |
| `eval` | evaluation framework |
| `server` | axum dev server with SSE + bearer auth + loopback guard |
| `cli` | embeddable `clap`-based CLI scaffolding |
| `macros` | the `#[tool]` proc-macro |
| `auth` | OAuth2 / ServiceAccount / API-key / HTTP credential flow |
| `openapi` | generate tools from an OpenAPI 3.x spec |
| `code-exec` | local-subprocess code executor |
| `code-exec-docker` | extra: ephemeral Docker container per call (`docker` on `$PATH`) |
| `a2a` | Agent-to-Agent JSON-RPC client + server bridge (spec-compliant) |
| `full` | enables all of the above |

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
use adk_rs::agents::LlmAgent;
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
use adk_rs::agents::LlmAgent;
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

## 🔐 Authenticated tools (`feature = "auth"`)

`adk-rs` ships full Python ADK parity for the credential lifecycle: OAuth 2.0 (authorization-code + PKCE, client-credentials, refresh-token), Service Account JWTs (Google-style RS256), API keys, and HTTP basic/bearer. When a tool declares `auth_config()`, the runner resolves the credential via `CredentialManager` *before* dispatch and injects it into `ToolContext::auth_credential`. If the underlying flow requires interactive consent (authorization-code), the agent emits a synthetic `adk_request_credential` function-call response and pauses; the caller resubmits the exchanged credential on the next turn.

```rust
use adk_rs::auth::{AuthConfig, AuthCredential, AuthScheme, ApiKeyLocation};

let cfg = AuthConfig::new(AuthScheme::ApiKey {
    location: ApiKeyLocation::Header,
    name: "X-API-Key".into(),
    description: None,
}).with_raw(AuthCredential::api_key("secret"));
```

## 🌐 OpenAPI tool generator (`feature = "openapi"`)

Point [`OpenAPIToolset`](src/tools/openapi/) at an OpenAPI 3.x spec and get one tool per operation. Security schemes from the spec map to `AuthConfig` automatically:

```rust
use adk_rs::auth::AuthCredential;
use adk_rs::tools::openapi::OpenAPIToolset;

let tools = OpenAPIToolset::from_path("petstore.yaml")?
    .with_credential("bearerAuth", AuthCredential::bearer(std::env::var("PETS_TOKEN")?))
    .into_tools();

let agent = LlmAgent::builder("pets")
    .model(model)
    .tools(tools)
    .build()?;
```

## 🐍 Code execution (`feature = "code-exec"`)

Attach a [`CodeExecutor`](src/code_exec/) to an `LlmAgent` and the agent will run any `ExecutableCode` parts the model emits, feeding `CodeExecutionResult` back on the next turn.

```rust
use adk_rs::code_exec::local::LocalCodeExecutor;
use std::sync::Arc;

let agent = LlmAgent::builder("coder")
    .model(model)
    .code_executor(Arc::new(LocalCodeExecutor::new())) // python3 on $PATH
    .build()?;
```

Two executors ship in the box:

- **`LocalCodeExecutor`** — spawns a child interpreter via `tokio::process` with a configurable timeout. Subprocess isolation only; **not a security boundary**.
- **`ContainerCodeExecutor`** (`feature = "code-exec-docker"`) — fresh ephemeral container per call, locked down by default: `--network=none`, `--read-only` rootfs, `--memory=256m`, `--cpus=1.0`, `--pids-limit=128`, `--cap-drop=ALL`, `--security-opt=no-new-privileges`, and `--user=65534:65534` (non-root). Every cap is exposed as a typed `with_*` builder method; relaxing the defaults is a deliberate act. Requires the `docker` CLI.

## 🔗 MCP — stdio and streamable HTTP (`feature = "mcp"`)

Connect to a local MCP server over stdio:

```rust
use adk_rs::mcp::{McpStdioParams, McpToolset};

let tools = McpToolset::stdio(McpStdioParams {
    command: "mcp-fs-server".into(),
    args: vec!["--root".into(), "/tmp/scratch".into()],
    ..Default::default()
})
.await?;
```

Or to a remote MCP server over the streamable-HTTP transport (single POST endpoint, response is `application/json` or SSE; `Mcp-Session-Id` is echoed automatically):

```rust
use adk_rs::mcp::{McpHttpParams, McpToolset};
use std::collections::HashMap;

let mut headers = HashMap::new();
headers.insert("Authorization".into(), format!("Bearer {}", std::env::var("MCP_TOKEN")?));
let tools = McpToolset::http(McpHttpParams {
    url: "https://mcp.example.com/v1".into(),
    headers,
    ..Default::default()
})
.await?;
```

If you pass a credential-bearing header against a non-loopback `http://` URL, construction refuses — see [Secure by default](#-secure-by-default) below.

## 🤖 A2A — Agent-to-Agent JSON-RPC (`feature = "a2a"`)

`adk-rs` ships a spec-compliant A2A surface so Rust agents can talk to other ADK agents (Python or Rust) over JSON-RPC. Both halves work:

**Expose a local `Runner` as an A2A endpoint:**

```rust
use adk_rs::a2a::{
    A2aServerConfig, A2aState, AgentCapabilities, AgentCard, serve,
};
use std::sync::Arc;

let card = AgentCard {
    name: "greeter".into(),
    description: "A friendly greeter".into(),
    url: "https://my-host/a2a/".into(),
    version: "0.1.0".into(),
    capabilities: AgentCapabilities { streaming: true, ..Default::default() },
    default_input_modes: vec!["text/plain".into()],
    default_output_modes: vec!["text/plain".into()],
    ..Default::default()
};
let state = A2aState::new(runner, A2aServerConfig::new(card).with_bearer_token("hunter2"));
serve("127.0.0.1:8080".parse()?, state).await?;
```

The router mounts:

- `GET /.well-known/agent.json` — discovery (the `AgentCard`).
- `POST /` — JSON-RPC: `message/send`, `message/stream` (SSE), `tasks/get`, `tasks/cancel`, `tasks/resubscribe`, `tasks/pushNotificationConfig/{set,get,list,delete}`.

Push-notification webhooks fan out via [`PushNotifier`](src/a2a/push_notifier.rs) — bodies match the `message/stream` envelope so the same receiver can consume both. Webhook URLs must be HTTPS or loopback.

**Call a remote A2A agent as a local `BaseAgent`:**

```rust
use adk_rs::a2a::{RemoteA2aAgent, RemoteA2aConfig};
use std::time::Duration;

let remote = RemoteA2aAgent::connect(RemoteA2aConfig {
    name: "fallback".into(), // overridden by the fetched agent card
    url: "https://peer.example.com/a2a/".into(),
    agent_card_url: Some("https://peer.example.com/.well-known/agent.json".into()),
    stream: true, // use message/stream over SSE
    timeout: Duration::from_secs(60),
    ..Default::default()
})
.await?;
```

Plug `remote` into any agent tree exactly like a local `LlmAgent` — `sub_agent(remote)`, `AgentTool::new(remote)`, etc.

## ⏹ Cancel and resume

Every invocation carries a cooperative [`CancellationToken`](src/core/cancel.rs). Agents check it between LLM calls and between sub-agents; when set, the stream ends with a `CANCELLED` event.

```rust
let handle = runner
    .start("user", None, content, RunConfig::default())
    .await?;
let inv_id = handle.invocation_id.clone();

// From anywhere:
runner.cancel(&inv_id);

// Or via A2A: `tasks/cancel` on the corresponding task id routes the
// cancel back through to the underlying runner invocation.
```

Resuming an interrupted conversation needs no special API — sessions are append-only event logs, so calling `runner.run(..., session_id, ...)` against an existing session replays the conversation. Auth-pending tool calls are resumed via the `adk_request_credential` synthetic flow.

## 🔒 Secure by default

A handful of guards trip when behaviour would be unsafe:

- **HTTPS-only credentials.** Provider clients (`Gemini`, `Anthropic`, `OpenAi`), `RestApiTool`, MCP HTTP, and the A2A client all refuse to send API keys / bearer tokens / cookies over plaintext HTTP. Loopback hosts are allowed for local mocks.
- **Loopback-only dev servers.** Both the `server` and A2A `serve` refuse non-loopback binds unless an auth token is configured (`AppState::with_bearer_token(...)`) or `ServeOptions::dangerously_allow_unauthenticated_remote` is opted in. CLI: `--auth-token` / `--dangerously-allow-unauthenticated-remote`.
- **Filesystem artifact paths.** `FileArtifactService::sanitize` collapses dot-only components so an attacker-controlled `app_name` / `user_id` / `session_id` / `filename` can't escape the artifact root via `..` segments.
- **Container code execution.** See the `ContainerCodeExecutor` defaults above — locked-down memory / CPU / pids caps, capability drops, non-root user.

## 🛠 Defining a tool

Add `#[tool]` to any async function. The macro derives a JSON schema from the arguments struct and returns a constructor for an `Arc<dyn Tool>` that can be handed to `LlmAgent::builder().tool(...)`.

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
    city: String,
    temp_c: f32,
    description: String,
}

/// Look up the current weather in `args.city`.
#[tool]
async fn get_weather(
    args: GetWeatherArgs,
    _ctx: &mut adk_rs::core::ToolContext,
) -> adk_rs::Result<WeatherReport> {
    Ok(WeatherReport {
        city: args.city,
        temp_c: 22.0,
        description: "sunny".into(),
    })
}
```

Attach it with `.tool(get_weather())` on the agent builder.

## 💻 Embedding the CLI

Unlike the Python CLI, Rust agents are statically linked. Build your own binary on top of `adk_rs::cli::App`:

```rust
use std::sync::Arc;

fn main() -> adk_rs::Result<()> {
    adk_rs::cli::App::new("my-app")
        .register("greeter", Arc::new(build_greeter()?))
        .run()
}
```

This gives you four subcommands out of the box:

```sh
my-app run --agent greeter "Hello!"     # single-turn invocation
my-app web --bind 127.0.0.1:8000        # axum dev server with SSE (loopback default)
my-app web --bind 0.0.0.0:8000 --auth-token "$ADK_WEB_TOKEN"   # non-loopback bind requires auth
my-app eval --agent greeter --set hello.evalset.json
my-app version
```

## 🌐 Dev web server

`adk-rs::server` exposes the runner over HTTP and Server-Sent Events for local testing and integration with web frontends. The `web` subcommand above starts it.

- Binds to `127.0.0.1` by default. Non-loopback addresses are refused unless you set a bearer token (`--auth-token` / `AppState::with_bearer_token`) or opt in via `--dangerously-allow-unauthenticated-remote`.
- When a token is set, every request must carry `Authorization: Bearer <token>`; comparisons are constant-time.

## 📊 Evaluating agents

The eval framework loads eval-set JSON files compatible with the Python ADK format, replays them through any `BaseAgent`, and scores with trajectory and response-match metrics. From the CLI:

```sh
my-app eval --agent greeter --set samples/hello_world.evalset.json
```

Programmatically:

```rust
let bytes = tokio::fs::read("hello_world.evalset.json").await?;
let set: adk_rs::eval::EvalSet = serde_json::from_slice(&bytes)?;
let runner = adk_rs::eval::EvalRunner::new(
    agent,
    "hello_world".into(),
    "eval-user",
    vec![
        Arc::new(adk_rs::eval::TrajectoryMatch::new(1.0)),
        Arc::new(adk_rs::eval::ResponseMatch::new(0.5)),
    ],
);
let report = runner.run_set(&set).await?;
```

## 📦 Module layout

`adk-rs` is a single crate organised by responsibility. Heavy dependencies (`sqlx`, `axum`, `reqwest`, OpenTelemetry, etc.) sit behind cargo features.

| Module | Feature gate | Responsibility |
|---|---|---|
| [`error`](src/error.rs) | always on | `Error` / `Result` and error codes. |
| [`transport_security`](src/transport_security.rs) | always on | `require_secure_url` — HTTPS-or-loopback guard shared by every credential-bearing client. |
| [`genai_types`](src/genai_types/) | always on | Wire-neutral data: `Content`, `Part`, `Schema`, `FunctionCall`, `GenerateContentConfig`, `Tool` (including Gemini server-side `googleSearch` / `urlContext` / `codeExecution`). |
| [`core`](src/core/) | always on | Domain primitives: `Event`, `Session`, `State`, `LlmRequest/Response`, `InvocationContext`, `CancellationToken`, service traits. |
| [`services::mem`](src/services/mem/) | always on | In-memory session, memory, artifact, and credential services. |
| [`services::fs`](src/services/fs.rs) | `fs` | Filesystem artifact service (path-traversal hardened). |
| [`services::sql`](src/services/sql/) | `sqlite` / `postgres` | SQL `SessionService` over `sqlx`. |
| [`providers::gemini`](src/providers/gemini/) | `gemini` | Gemini REST + SSE provider; HTTPS-only base URL. |
| [`providers::anthropic`](src/providers/anthropic/) | `anthropic` | Anthropic Messages API + SSE provider; HTTPS-only base URL. |
| [`providers::openai`](src/providers/openai/) | `openai` | OpenAI-compatible provider (Azure / Ollama / Groq via base-URL); HTTPS-only base URL. |
| [`tools`](src/tools/) | always on | `Tool` trait, `FunctionTool`, built-ins (`transfer_to_agent`, `exit_loop`, `google_search`, `url_context`, `built_in_code_execution`, `load_artifacts`, `load_memory`, `get_user_choice`, `agent_tool`, `LongRunningFunctionTool`). |
| [`tools::openapi`](src/tools/openapi/) | `openapi` | `OpenAPIToolset` — generate `RestApiTool`s from an OpenAPI 3.x spec. |
| [`agents`](src/agents/) | always on | `BaseAgent`, `LlmAgent`, `SequentialAgent`, `ParallelAgent`, `LoopAgent`. All observe `InvocationContext::cancellation`. |
| [`auth`](src/auth/) | types always on, flow gated on `auth` | `AuthCredential`, `AuthScheme`, `AuthConfig`, `CredentialService`, `CredentialManager`, OAuth2 `AuthHandler`, `AuthPreprocessor`. |
| [`code_exec`](src/code_exec/) | `code-exec` (+ `code-exec-docker`) | `CodeExecutor` trait; `LocalCodeExecutor`, locked-down `ContainerCodeExecutor`. |
| [`runner`](src/runner/) | always on | Orchestration: `Runner::start` returns a `RunningInvocation` handle; `Runner::cancel(invocation_id)` halts in-flight agents. |
| [`mcp`](src/mcp/) | `mcp` | MCP stdio + streamable HTTP transports, `McpClient`, `McpToolset`. |
| [`a2a`](src/a2a/) | `a2a` | Spec-compliant A2A JSON-RPC: types, `TaskService` + `InMemoryTaskService`, `PushNotifier`, `RemoteA2aAgent` client, axum server bridge, agent-card discovery. |
| [`telemetry`](src/telemetry.rs) | `telemetry` (+ `otel`) | `tracing-subscriber` setup with optional OTLP export. |
| [`eval`](src/eval/) | `eval` | Eval-set IO and metrics. |
| [`server`](src/server/) | `server` | `axum` dev server with SSE, bearer-token auth, and loopback-default bind guard. |
| [`cli`](src/cli.rs) | `cli` | Embeddable CLI scaffolding. |

The `#[tool]` proc-macro lives in a sibling crate, [`adk-rs-macros`](adk-rs-macros/), which is required by the Rust compiler to be its own crate. Enable it via the `macros` feature.

## 🧩 Examples

Runnable demos live under [`examples/`](examples/):

- [`gemini_chat`](examples/gemini_chat.rs) — minimal single-agent loop.
- [`weather_agent`](examples/weather_agent.rs) — `#[tool]`-defined function tool.
- [`three_providers`](examples/three_providers.rs) — the same prompt against Gemini, Claude, and OpenAI.
- [`code_agent`](examples/code_agent.rs) — agent that emits shell snippets, runner executes them via `LocalCodeExecutor`.

```sh
cargo run --example weather_agent --features "gemini,macros"
cargo run --example code_agent --features "code-exec,testing"
```

## 📖 Documentation site

A full documentation site (every module, feature flag, example, and guides) lives under [`docs/`](docs/) as a standalone Next.js app:

```sh
cd docs && npm install && npm run dev   # http://localhost:3000
```

The `docs/` folder is excluded from the published crate (`package.exclude` in `Cargo.toml`) and is not a workspace member — it never affects `cargo build`, `cargo test`, or `cargo publish`.

## 🤝 Contributing

Bug reports, feature requests, and pull requests are welcome. Before submitting:

```sh
cargo fmt --all
cargo clippy --all-features --all-targets
cargo test --all-features
```

Please open an issue before starting on substantial changes.

## 📄 License

Licensed under the Apache License, Version 2.0 — see [LICENSE](LICENSE).
