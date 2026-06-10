import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'guides/production-deploy',
  title: 'Guide: Production deployment',
  description:
    'Take an adk-rs agent to production with a lean feature set, JSON telemetry with OTLP export, an authenticated HTTP server, a hardened container image, and runtime cost controls.',
  blocks: [
    {
      kind: 'lede',
      text: 'An adk-rs agent ships as one static binary, which makes deployment refreshingly boring: pick the features you actually use, turn on structured telemetry, serve behind a bearer token, and wrap it in a small container. This guide walks that path and points out the security guards you will meet along the way.',
    },
    { kind: 'h2', text: '1. Choose a lean feature set' },
    {
      kind: 'p',
      text: 'Default features are empty and `full` exists for development — production builds should name exactly what they need so heavy dependencies (`sqlx`, `axum`, OpenTelemetry) only compile when used. See [Installation](/docs/installation) for the full matrix. A typical serving stack:',
    },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[dependencies]
adk-rs = { version = "0.3", features = [
  "gemini",     # your provider(s)
  "sqlite",     # durable sessions
  "fs",         # filesystem artifacts
  "server",     # axum HTTP server
  "otel",       # OTLP trace export (implies "telemetry")
] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }`,
    },
    {
      kind: 'p',
      text: 'For smaller, faster binaries, set `lto = "thin"`, `codegen-units = 1`, and `strip = "symbols"` in your `[profile.release]`.',
    },
    { kind: 'h2', text: '2. Initialize telemetry' },
    {
      kind: 'p',
      text: 'Call [`adk_rs::telemetry::init`](/docs/telemetry) once at process start. It installs a `tracing` subscriber with an env-filter (a set `RUST_LOG` overrides the configured filter) and, when the `otel` feature is on and an endpoint is provided, a batching OTLP HTTP span exporter. The call is idempotent.',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `use adk_rs::telemetry::{self, LogFormat, TelemetryConfig};

telemetry::init(TelemetryConfig {
    filter: Some("info,adk_rs=debug".into()),
    format: LogFormat::Json, // newline-delimited JSON for log aggregators
    otlp_endpoint: std::env::var("OTLP_ENDPOINT").ok(),
    service_name: Some("research-agent".into()),
})?;`,
    },
    { kind: 'h2', text: '3. Build the binary: CLI scaffold or direct server' },
    {
      kind: 'p',
      text: 'The quickest path is [`adk_rs::cli::App`](/docs/cli) (feature `cli`, which transitively enables `telemetry`, `server`, and `eval`): register your agents and you get `run`, `web`, `eval`, and `version` subcommands, with telemetry initialized from `--log`/`--log-format`.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/main.rs (CLI scaffold)',
      code: `use std::sync::Arc;

fn main() -> adk_rs::Result<()> {
    adk_rs::cli::App::new("research")
        .register("pipeline", Arc::new(build_pipeline()?))
        .run()
}`,
    },
    {
      kind: 'code',
      lang: 'bash',
      code: `./research web --bind 0.0.0.0:8000 \\
  --auth-token "$ADK_WEB_TOKEN" \\
  --allow-origins https://app.example.com`,
    },
    {
      kind: 'callout',
      tone: 'warn',
      text: 'The CLI scaffold builds its runners on `InMemorySessionService` — fine for a dev box, wrong for production. For durable serving, construct your own `Runner` (with the SQLite session service from [Guide: Persistent sessions](/docs/guides/persistent-sessions)) and use the server module directly. The [`server`](/docs/server) module takes a map of agent name → `Runner` and serves the `adk api_server`-compatible endpoint surface (REST + SSE) that Google’s adk-web UI speaks; `AppState::with_bearer_token` enforces `Authorization: Bearer <token>` on every route with a constant-time comparison, and `with_allow_origins` emits CORS headers for your web frontend’s origin.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/main.rs (direct server)',
      code: `use adk_rs::server::{self, AppState};
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let runner = Arc::new(build_runner().await?); // your durable Runner
    let mut runners = HashMap::new();
    runners.insert("pipeline".to_string(), runner);

    let token = std::env::var("ADK_WEB_TOKEN").expect("ADK_WEB_TOKEN must be set");
    let state = AppState::with_bearer_token(Arc::new(runners), token)
        .with_allow_origins(["https://app.example.com".to_string()]);

    server::serve("0.0.0.0:8000".parse().unwrap(), state).await
}`,
    },
    { kind: 'h2', text: '4. The non-loopback guard, satisfied properly' },
    {
      kind: 'p',
      text: 'Binding anything other than `127.0.0.1`/`::1` without authentication makes `serve` return a config error instead of starting — otherwise anyone reachable on the network could drive your agents and read every session. Two legitimate ways forward, one escape hatch:',
    },
    {
      kind: 'list',
      items: [
        '**Set a bearer token** — `AppState::with_bearer_token(runners, token)` (or `--auth-token` / `ADK_WEB_TOKEN` on the CLI). This is the right answer for direct exposure.',
        '**Stay on loopback** and put your own authenticating reverse proxy (nginx, Envoy, a cloud LB) in front. The guard never triggers for `127.0.0.1`.',
        '`ServeOptions::dangerously_allow_unauthenticated_remote` (CLI: `--dangerously-allow-unauthenticated-remote`) disables the guard. Reserve it for isolated networks you fully control — the server still logs a loud warning.',
      ],
    },
    { kind: 'h2', text: '5. A hardened Dockerfile' },
    {
      kind: 'p',
      text: 'adk-rs uses rustls throughout (no OpenSSL to install), so the runtime stage can be a distroless image — CA certificates included, no shell, non-root user. Inside the container the server must bind `0.0.0.0` to be reachable, which is exactly why the bearer token from step 4 is non-negotiable. Mount a volume for the SQLite file (e.g. `sqlite:///data/adk.db?mode=rwc`) and the artifact root, and inject secrets (`GOOGLE_API_KEY`, `ADK_WEB_TOKEN`, `OTLP_ENDPOINT`) from your orchestrator’s secret store — never bake them into the image.',
    },
    {
      kind: 'code',
      lang: 'text',
      title: 'Dockerfile',
      code: `FROM rust:1.85-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/research /usr/local/bin/research
ENTRYPOINT ["/usr/local/bin/research"]`,
    },
    { kind: 'h2', text: '6. Operational budgets: calls, caching, compaction' },
    {
      kind: 'p',
      text: 'Three knobs keep long-lived deployments fast and affordable. Cap the per-invocation LLM-call budget with [`RunConfig`](/docs/runner) so a runaway tool loop fails fast instead of burning quota; enable provider-side context caching and automatic event compaction on the runner:',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `use adk_rs::core::{ContextCacheConfig, RunConfig};
use adk_rs::genai_types::Content;
use adk_rs::runner::{EventsCompactionConfig, Runner};

let runner = Runner::builder()
    .app_name("research")
    .agent(agent)
    .session_service(sessions)
    .context_cache_config(ContextCacheConfig::default())
    .compaction(EventsCompactionConfig::new(summarizer_model.clone()))
    .build()?;

let cfg = RunConfig {
    max_llm_calls: Some(25), // hard per-invocation budget
    ..RunConfig::default()
};
let mut events = runner
    .run_with("alice", Some("alice-main"), Content::user_text(prompt), cfg)
    .await?;`,
    },
    {
      kind: 'list',
      items: [
        '[Context caching](/docs/context-caching) — pair `context_cache_config` with `LlmAgent::builder().static_instruction(...)` so the system prefix stays byte-identical across turns; defaults are a 30-minute TTL refreshed every 10 calls.',
        '[Event compaction](/docs/event-compaction) — `EventsCompactionConfig::new(model)` summarizes older events with an LLM after invocations complete (default: every 5 invocations, 2 events of overlap; tune with `.compaction_interval(n)` and `.overlap_size(n)`), so multi-week sessions stop growing the prompt without bound.',
        '[Cancellation and resume](/docs/cancellation-and-resume) — `runner.start(...)` returns a `RunningInvocation` handle whose id you can `cancel` from an admin endpoint.',
      ],
    },
    { kind: 'h2', text: 'Where next' },
    {
      kind: 'list',
      items: [
        '[Server](/docs/server) — the full endpoint surface and SSE wire format.',
        '[Telemetry](/docs/telemetry) — span structure and what gets exported via OTLP.',
        '[Security](/docs/security) — every guard in one place.',
        '[Eval](/docs/eval) — gate deploys on eval-set scores with the `eval` subcommand.',
      ],
    },
  ],
};
