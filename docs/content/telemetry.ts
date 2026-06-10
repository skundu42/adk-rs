import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'telemetry',
  title: 'Telemetry',
  description:
    'Configure tracing output and optional OTLP span export with TelemetryConfig and a single init call.',
  srcPath: 'src/telemetry.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs instruments itself with the `tracing` crate. The `telemetry` feature adds a one-shot `init` that wires a `tracing-subscriber` stack — an env-style filter plus a formatted stderr layer — and the `otel` feature extends it with an OpenTelemetry OTLP export pipeline.',
    },
    { kind: 'h2', text: 'TelemetryConfig' },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Meaning'],
      rows: [
        ['`filter`', '`Option<String>`', '`RUST_LOG`-style filter, e.g. `adk_rs=debug,info`. Defaults to `info`. A set `RUST_LOG` environment variable takes precedence over this field.'],
        ['`format`', '`LogFormat`', 'stderr output format: `Compact` (default), `Pretty`, or `Json` (newline-delimited, for log aggregators).'],
        ['`otlp_endpoint`', '`Option<String>`', 'OTLP HTTP endpoint URL. Only used when the `otel` feature is enabled; ignored otherwise.'],
        ['`service_name`', '`Option<String>`', '`service.name` on the OTel resource. Defaults to `adk-rs`.'],
      ],
    },
    { kind: 'h2', text: 'init' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn init(cfg: TelemetryConfig) -> Result<()>',
          desc: 'Initialise tracing once at process start. Idempotent — a second call is a no-op, so libraries and tests can call it defensively. Filter resolution order: `RUST_LOG` env var, then `cfg.filter`, then `info`.',
        },
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Plain structured logging',
      code: `use adk_rs::telemetry::{LogFormat, TelemetryConfig, init};

init(TelemetryConfig {
    filter: Some("adk_rs=debug,info".into()),
    format: LogFormat::Json,
    ..TelemetryConfig::default()
})?;`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'The [embedded CLI](/docs/cli) calls `init` for you, mapping `--log` to `filter` (env: `ADK_LOG`) and `--log-format` to `format`.',
    },
    { kind: 'h2', text: 'What gets traced' },
    {
      kind: 'p',
      text: 'The crate creates spans via `#[tracing::instrument]` at the three layers you most often need to correlate:',
    },
    {
      kind: 'list',
      items: [
        '**Runner** — `Runner::run` opens a span carrying `app` and `agent` fields, so every event of a turn nests under one root span.',
        '**Agents** — `LlmAgent::run` opens a span with `agent` and `invocation` (the invocation id) fields.',
        '**Models** — each provider’s `generate_content` (Gemini, Anthropic, OpenAI-compatible) opens a span with a `model` field, capturing per-call latency. Request bodies are skipped from the span fields.',
      ],
    },
    {
      kind: 'p',
      text: 'Beneath the spans, the crate logs events at conventional levels: `warn` for recoverable conditions (webhook delivery failures, non-loopback binds, schema fallbacks), `debug` for protocol details, and `error` for stream failures surfaced to HTTP clients. Tool execution is visible through the agent span’s events rather than a dedicated per-tool span.',
    },
    { kind: 'h2', text: 'OTLP export (feature otel)' },
    {
      kind: 'p',
      text: 'With `otel` enabled and `otlp_endpoint` set, `init` builds an `opentelemetry-otlp` **HTTP span exporter** pointed at the endpoint, wraps it in a batch exporter on the Tokio runtime, attaches a resource with your `service_name`, and layers `tracing-opentelemetry` into the subscriber. Every `tracing` span above — runner, agent, model — is then exported as an OTel trace. When `otlp_endpoint` is `None`, no OTel layer is installed even with the feature on.',
    },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[dependencies]
adk-rs = { version = "0.6", features = ["gemini", "otel"] }
# "otel" implies "telemetry"`,
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Exporting to an OTLP collector',
      code: `use adk_rs::telemetry::{TelemetryConfig, init};

init(TelemetryConfig {
    filter: Some("info".into()),
    otlp_endpoint: Some("http://localhost:4318/v1/traces".into()),
    service_name: Some("weather-agent".into()),
    ..TelemetryConfig::default()
})?;`,
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Embedded CLI](/docs/cli) — logging flags that feed `TelemetryConfig`.',
        '[Runner](/docs/runner) — the orchestration layer the root spans wrap.',
        '[Guides: production deploy](/docs/guides/production-deploy) — telemetry in a deployed service.',
      ],
    },
  ],
};
