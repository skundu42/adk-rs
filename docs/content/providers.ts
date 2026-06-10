import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'providers',
  title: 'Providers',
  description:
    'The Gemini, Anthropic, and OpenAI-compatible Model implementations, their configuration, environment variables, feature flags, and transport-security guarantees.',
  srcPath: 'src/providers/mod.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs ships three provider clients — `Gemini`, `Anthropic`, and `OpenAi` — each behind its own cargo feature and each implementing the same [`Model`](/docs/models) trait. Because agents only see `Arc<dyn Model>`, switching providers is a one-line change.',
    },
    { kind: 'h2', text: 'At a glance' },
    {
      kind: 'table',
      head: ['Provider', 'Feature flag', 'Env var(s)', '`supported_models`'],
      rows: [
        ['`providers::gemini::Gemini`', '`gemini`', '`GOOGLE_API_KEY`', '`gemini-*`'],
        ['`providers::anthropic::Anthropic`', '`anthropic`', '`ANTHROPIC_API_KEY`', '`claude-*`'],
        [
          '`providers::openai::OpenAi`',
          '`openai`',
          '`OPENAI_API_KEY`, optional `OPENAI_BASE_URL`',
          '`openai/*`, `gpt-*`, `o1-*`, `o3-*`, `azure/*`, `ollama/*`, `groq/*`',
        ],
      ],
    },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[dependencies]
adk-rs = { version = "0.6", features = ["gemini", "anthropic", "openai"] }`,
    },
    { kind: 'h2', text: 'Retries' },
    {
      kind: 'p',
      text: 'Every provider client retries transient failures automatically: 429 rate limits (honouring a `Retry-After` header up to 60s), 408/409/5xx responses, and connect/timeout transport errors. Backoff is exponential with full jitter. The policy lives on each config as a `retry: RetryConfig` field; the default mirrors the official provider SDKs — 2 retries, 500ms initial backoff, 8s cap. Other 4xx errors (bad request, auth) fail immediately.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Tuning or disabling the retry policy',
      code: `use adk_rs::core::RetryConfig;
use adk_rs::providers::gemini::{Gemini, GeminiConfig};
use std::time::Duration;

let model = Gemini::new("gemini-2.5-flash", GeminiConfig {
    api_key: std::env::var("GOOGLE_API_KEY")?,
    retry: RetryConfig {
        max_retries: 5,
        initial_backoff: Duration::from_millis(250),
        max_backoff: Duration::from_secs(30),
        ..RetryConfig::default()
    },
    // or: retry: RetryConfig::disabled(),
    ..GeminiConfig::default()
})?;`,
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'For streaming calls the retry covers connection establishment only — once the first SSE byte has been read, a mid-stream failure surfaces as a stream error rather than a silent replay.',
    },
    { kind: 'h2', text: 'Gemini' },
    {
      kind: 'p',
      text: 'The Gemini client speaks the `generateContent` REST API and real SSE streaming. `Gemini::from_env(model_name)` reads `$GOOGLE_API_KEY`; `Gemini::new(model_name, GeminiConfig)` gives full control. `GeminiConfig` has five fields: `base_url` (default `https://generativelanguage.googleapis.com`), `api_version` (default `v1beta`), `api_key`, `timeout` (default 60 s), and `retry`. `timeout` is the total timeout for **non-streaming** requests only — streaming (SSE) requests are exempt, with just a 10-second connect timeout, so a long generation is never cut off mid-stream; the same rule applies to all three providers. The API key travels in the `x-goog-api-key` header.',
    },
    {
      kind: 'list',
      items: [
        '**Streaming** — `stream_generate_content` POSTs to `:streamGenerateContent?alt=sse` and decodes the SSE chunks into a stream of `LlmResponse` values.',
        '**Server-side built-in tools** — when the request config carries `Tool::GoogleSearch {}`, `Tool::UrlContext {}`, or `Tool::CodeExecution {}` (injected by the [Gemini built-in tool handles](/docs/builtin-tools)), Gemini runs search grounding, URL grounding, or sandboxed Python on Google’s servers.',
        '**Context caching** — if the request carries a `ContextCacheConfig`, the client creates a server-side `cachedContents` entry for the stable prefix (system instruction + tools), reuses it on later calls keyed by a fingerprint, and transparently retries without the cache if the server rejects a stale entry. See [Context caching](/docs/context-caching).',
        '**Live API** — with the `live` feature, `Gemini::connect_live` opens a bidirectional WebSocket session for realtime text and audio. See [Gemini Live](/docs/live).',
      ],
    },
    { kind: 'h2', text: 'Anthropic' },
    {
      kind: 'p',
      text: 'The Anthropic client targets the Messages API (`POST {base_url}/v1/messages`) with `x-api-key` and `anthropic-version` headers. `Anthropic::from_env(model_name)` reads `$ANTHROPIC_API_KEY`; `AnthropicConfig` exposes `base_url` (default `https://api.anthropic.com`), `anthropic_version` (default `2023-06-01`), `api_key`, `timeout`, and `retry`.',
    },
    {
      kind: 'list',
      items: [
        '**Streaming** — native SSE: text and thinking deltas are emitted as partial chunks the moment they arrive, tool-call arguments accumulate across `input_json_delta` fragments and surface as one complete `FunctionCall`, and the final chunk carries the stop reason and usage.',
        '**Multimodal input** — `Part::InlineData` images become base64 `image` blocks, inline PDFs become `document` blocks, and `https://` `Part::FileData` references become URL sources. Unsupported parts are dropped with a warning, never silently.',
        '**Prompt caching** — a [`ContextCacheConfig`](/docs/context-caching) on the request becomes a `cache_control` breakpoint on the system block (or the last tool when there is no system instruction), so Anthropic caches the stable prefix server-side. Cache activity surfaces on `event.response.cache_metadata` and `usage_metadata.cached_content_token_count`.',
        '**Extended thinking** — `GenerateContentConfig.thinking_config.thinking_budget` maps to the Messages API `thinking` parameter (`{"type": "enabled", "budget_tokens": N}`). The default `max_tokens` grows to budget + 2048 so thinking never starves the answer (an explicit `max_output_tokens` is always respected), and `temperature`/`top_p`/`top_k` are dropped while thinking is enabled — the API rejects them together. Thinking blocks round-trip with their cryptographic signature, `redacted_thinking` blocks are preserved as `Part::RedactedThought`, and streaming handles `thinking_delta` + `signature_delta`.',
        '**Forward compatibility** — unknown content-block types in responses are skipped instead of failing the whole response; the `refusal` stop reason maps to `FinishReason::Safety` and `pause_turn` to `Stop`.',
      ],
    },
    { kind: 'h2', text: 'OpenAI (and Azure, Ollama, Groq)' },
    {
      kind: 'p',
      text: 'The `OpenAi` client speaks the `chat/completions` protocol, which makes it the bridge to every OpenAI-compatible endpoint. `OpenAi::from_env(model_name)` reads `$OPENAI_API_KEY` and honours `$OPENAI_BASE_URL` (default `https://api.openai.com/v1`). `OpenAiConfig` adds `api_version` (appended as Azure’s `?api-version=` query parameter) and `organization` (sent as the `OpenAI-Organization` header). The key travels as `Authorization: Bearer ...`.',
    },
    {
      kind: 'list',
      items: [
        '**Streaming** — native SSE with `stream_options: {include_usage: true}`: content deltas stream as partial chunks; fragment-wise tool calls are reassembled by index and emitted complete in the final chunk alongside the finish reason and usage.',
        '**Multimodal input** — text-only user messages stay plain strings; messages with images switch to the content-parts form, mapping inline images to `data:` URI `image_url` parts and `https://` image references to plain `image_url` parts.',
        '**Reasoning models** — `max_output_tokens` is sent as `max_completion_tokens` for the o-series / gpt-5 family (which reject the deprecated `max_tokens` with a 400) and as `max_tokens` for everything else, keeping older OpenAI-compatible servers working.',
      ],
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'To verify wire compatibility against the live APIs with your own keys, run `cargo run --example compat_check --features "anthropic,openai"` — it exercises generation, streaming, tool calling, structured output, image input, and prompt-cache breakpoints, printing PASS/FAIL per check.',
    },
    {
      kind: 'code',
      lang: 'bash',
      title: 'Pointing the same client at different backends',
      code: `# Stock OpenAI
export OPENAI_API_KEY=sk-...

# Local Ollama (loopback HTTP is allowed)
export OPENAI_BASE_URL=http://localhost:11434/v1
export OPENAI_API_KEY=ollama   # any non-empty value

# Groq
export OPENAI_BASE_URL=https://api.groq.com/openai/v1
export OPENAI_API_KEY=gsk_...`,
    },
    { kind: 'h2', text: 'Embedders' },
    {
      kind: 'p',
      text: 'Both the Gemini and OpenAI features ship an [`Embedder`](/docs/memory) implementation for semantic memory: `GeminiEmbedder` (the `batchEmbedContents` API, e.g. `gemini-embedding-001`) and `OpenAiEmbedder` (the `/embeddings` endpoint, e.g. `text-embedding-3-small` — also reaches Azure and Ollama via `OPENAI_BASE_URL`). Each has the same `from_env` / config constructors as its chat sibling and shares the retry policy. Plug either into [`VectorMemoryService`](/docs/memory).',
    },
    { kind: 'h2', text: 'Transport security: HTTPS or loopback' },
    {
      kind: 'p',
      text: 'Every provider constructor validates its base URL with `transport_security::require_secure_url` before building the HTTP client. The rule: the destination must be `https://`, or a plaintext-HTTP **loopback** host (`localhost`, any `127.0.0.0/8` address, or `[::1]`). A public `http://` base URL is rejected with a configuration error rather than silently shipping your API key in cleartext. Loopback stays allowed so local mocks, Ollama, and test servers keep working. All three clients also disable HTTP redirects (`redirect::Policy::none()`) — reqwest re-sends custom headers on redirect, so a redirecting endpoint could otherwise exfiltrate the API key to another host.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Rejected at construction time',
      code: `use adk_rs::providers::openai::{OpenAi, OpenAiConfig};

let err = OpenAi::new(
    "gpt-4o-mini",
    OpenAiConfig {
        base_url: "http://api.example.com/v1".into(), // plaintext, non-loopback
        api_key: "sk-...".into(),
        ..OpenAiConfig::default()
    },
)
.unwrap_err(); // "base_url must be https:// or point to a loopback host ..."`,
    },
    { kind: 'h2', text: 'Swapping providers behind Arc<dyn Model>' },
    {
      kind: 'p',
      text: 'The pattern from the repo’s `three_providers` example: build whichever clients have credentials, then hand any of them to the same agent.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'examples/three_providers.rs (condensed)',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::core::Model;
use adk_rs::providers::anthropic::Anthropic;
use adk_rs::providers::gemini::Gemini;
use adk_rs::providers::openai::OpenAi;
use std::sync::Arc;

let mut models: Vec<(&str, Arc<dyn Model>)> = Vec::new();
if std::env::var("GOOGLE_API_KEY").is_ok() {
    models.push(("Gemini", Arc::new(Gemini::from_env("gemini-2.5-flash")?)));
}
if std::env::var("ANTHROPIC_API_KEY").is_ok() {
    models.push(("Claude", Arc::new(Anthropic::from_env("claude-3-5-sonnet")?)));
}
if std::env::var("OPENAI_API_KEY").is_ok() {
    models.push(("OpenAI", Arc::new(OpenAi::from_env("gpt-4o-mini")?)));
}

for (label, model) in models {
    let agent = LlmAgent::builder("greeter")
        .model(model) // same builder, any provider
        .instruction("Be concise.")
        .build()?;
    println!("=== {label} ===");
    // ... run via Runner as usual
}`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'Errors are uniform across providers: transport failures surface as `ProviderError::Transport`, non-2xx responses as `ProviderError::Http { status, body }`, and missing keys as `ProviderError::Auth`. See [Errors](/docs/errors).',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Models & requests](/docs/models) — the `Model` trait and request/response types.',
        '[Installation](/docs/installation) — feature flags and credentials setup.',
        '[Three providers example](/docs/examples/three-providers) — the full runnable walkthrough.',
        '[Security](/docs/security) — the complete transport-security posture.',
      ],
    },
  ],
};
