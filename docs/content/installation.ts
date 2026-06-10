import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'installation',
  title: 'Installation',
  description:
    'How to add adk-rs to a project, which cargo feature flags exist, and which environment variables each provider reads.',
  srcPath: 'Cargo.toml',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs ships as a single crate whose default feature set is empty. You opt in to providers, storage backends, and subsystems with cargo features, so a minimal agent compiles only the dependencies it actually uses.',
    },
    { kind: 'h2', text: 'Add the dependency' },
    {
      kind: 'p',
      text: 'Add `adk-rs` with the features you need. Almost every program wants at least one provider; most also want `macros` for the [`#[tool]`](/docs/function-tools) proc-macro.',
    },
    {
      kind: 'code',
      lang: 'bash',
      code: `cargo add adk-rs --features gemini,macros
cargo add tokio --features macros,rt-multi-thread
cargo add futures`,
    },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[dependencies]
adk-rs = { version = "0.5", features = ["gemini", "macros"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"`,
    },
    {
      kind: 'callout',
      tone: 'warn',
      title: 'Default features are empty',
      text: '`default = []` in the crate manifest. Adding `adk-rs` with no features gives you the core types (`agents`, `core`, `genai_types`, `tools`, `runner`, in-memory services) but **no LLM provider** — `adk_rs::providers` stays empty until you enable `gemini`, `anthropic`, or `openai`.',
    },
    { kind: 'h2', text: 'Feature flags' },
    {
      kind: 'p',
      text: 'Every flag defined in the `[features]` section of `Cargo.toml`, and the optional dependencies it activates. Heavy dependencies (`sqlx`, `axum`, `reqwest`, OpenTelemetry) only compile when the matching feature is on.',
    },
    {
      kind: 'table',
      head: ['Feature', 'What it enables', 'Optional dependencies'],
      rows: [
        ['`gemini`', 'Gemini REST + SSE provider (`providers::gemini`).', '`reqwest`, `eventsource-stream`, `url`'],
        ['`anthropic`', 'Anthropic Messages API + SSE provider.', '`reqwest`, `eventsource-stream`, `url`'],
        ['`openai`', 'OpenAI-compatible provider; serves Azure OpenAI, Ollama, and Groq via base-URL override.', '`reqwest`, `eventsource-stream`'],
        ['`live`', '[Gemini Live API](/docs/live) — bidirectional WebSocket streaming; implies `gemini`.', '`tokio-tungstenite`'],
        ['`fs`', 'Filesystem [artifact service](/docs/artifacts), path-traversal hardened.', 'none'],
        ['`sqlite`', 'SQL `SessionService` backed by SQLite.', '`sqlx` (sqlite)'],
        ['`postgres`', 'SQL `SessionService` backed by PostgreSQL.', '`sqlx` (postgres)'],
        ['`mcp`', '[MCP toolset](/docs/mcp) — stdio and streamable-HTTP transports.', '`reqwest`, `eventsource-stream`, `url`'],
        ['`telemetry`', '[`tracing` setup](/docs/telemetry) via `tracing-subscriber`.', '`tracing-subscriber`'],
        ['`otel`', 'OpenTelemetry OTLP export; implies `telemetry`.', '`opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `tracing-opentelemetry`'],
        ['`eval`', '[Evaluation framework](/docs/eval) — eval-set replay and scoring.', 'none'],
        ['`server`', '[axum dev server](/docs/server) with SSE, bearer auth, loopback guard.', '`axum`, `tower`, `tower-http`, `http`'],
        ['`cli`', 'Embeddable [clap CLI](/docs/cli); implies `telemetry`, `server`, `eval`.', '`clap`'],
        ['`macros`', 'The `#[tool]` proc-macro from the sibling `adk-rs-macros` crate.', '`adk-rs-macros`'],
        ['`auth`', '[Credential flows](/docs/auth): OAuth2, service-account JWTs, API keys.', '`reqwest`, `url`, `oauth2`, `jsonwebtoken`, `rand`'],
        ['`openapi`', '[OpenAPI 3.x tool generator](/docs/openapi-tools).', '`reqwest`, `url`, `openapiv3`, `serde_yaml`'],
        ['`code-exec`', 'Local-subprocess [code executor](/docs/code-execution).', 'none'],
        ['`code-exec-docker`', 'Locked-down Docker container executor; implies `code-exec`.', 'none (`docker` CLI on `$PATH`)'],
        ['`a2a`', '[Agent-to-Agent](/docs/a2a) JSON-RPC client + server bridge.', '`reqwest`, `url`, `eventsource-stream`, `axum`, `tower`, `tower-http`, `http`'],
        ['`testing`', 'Test helpers such as `adk_rs::core::testing::MockModel` for use outside the crate. See [Testing](/docs/testing).', 'none'],
        ['`full`', 'Convenience superset — see the note below.', 'everything above'],
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      title: '`full` is not literally everything',
      text: '`full` enables `gemini`, `anthropic`, `openai`, `live`, `fs`, `sqlite`, `mcp`, `telemetry`, `eval`, `server`, `cli`, `macros`, `auth`, `openapi`, `code-exec`, `code-exec-docker`, `a2a`, and `testing` — but **not** `postgres` or `otel`. Add those two explicitly if you need them.',
    },
    { kind: 'h2', text: 'Provider credentials' },
    {
      kind: 'p',
      text: 'Each provider has a `from_env` constructor that reads its API key from the environment and fails with a configuration error when the variable is missing or empty.',
    },
    {
      kind: 'table',
      head: ['Provider', 'Constructor', 'Environment variables'],
      rows: [
        ['Gemini', '`Gemini::from_env(model)`', '`GOOGLE_API_KEY`'],
        ['Anthropic', '`Anthropic::from_env(model)`', '`ANTHROPIC_API_KEY`'],
        ['OpenAI-compatible', '`OpenAi::from_env(model)`', '`OPENAI_API_KEY`, plus optional `OPENAI_BASE_URL` (defaults to `https://api.openai.com/v1`)'],
      ],
    },
    {
      kind: 'p',
      text: 'Setting `OPENAI_BASE_URL` is how you point the OpenAI client at Azure OpenAI, Ollama, Groq, or any other OpenAI-compatible endpoint. All provider clients enforce HTTPS-or-loopback for the base URL — see [Security](/docs/security).',
    },
    { kind: 'h2', text: 'Toolchain requirements' },
    {
      kind: 'list',
      items: [
        '**MSRV: Rust 1.85** (`rust-version = "1.85"` in the manifest), **edition 2024**.',
        'The repository pins `channel = "1.85.0"` in `rust-toolchain.toml`, with `rustfmt`, `clippy`, and `rust-src` components.',
        'The async runtime is `tokio`; event streams are ordinary `futures::Stream`s, so you also want the `futures` crate for `StreamExt`.',
      ],
    },
    {
      kind: 'callout',
      tone: 'tip',
      title: 'docs.rs',
      text: 'The published API reference on docs.rs is built with `all-features = true` (`[package.metadata.docs.rs]`), so feature-gated modules like `providers`, `mcp`, and `a2a` are all visible there even though they are off by default.',
    },
    { kind: 'h2', text: 'Where next' },
    {
      kind: 'list',
      items: [
        '[Quickstart](/docs/quickstart) — build and run your first agent end to end.',
        '[Providers](/docs/providers) — Gemini, Anthropic, and OpenAI-compatible clients in depth.',
        '[Introduction](/docs/introduction) — design overview and crate layout.',
      ],
    },
  ],
};
