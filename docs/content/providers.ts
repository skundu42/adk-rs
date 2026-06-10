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
adk-rs = { version = "0.3", features = ["gemini", "anthropic", "openai"] }`,
    },
    { kind: 'h2', text: 'Gemini' },
    {
      kind: 'p',
      text: 'The Gemini client speaks the `generateContent` REST API and real SSE streaming. `Gemini::from_env(model_name)` reads `$GOOGLE_API_KEY`; `Gemini::new(model_name, GeminiConfig)` gives full control. `GeminiConfig` has four fields: `base_url` (default `https://generativelanguage.googleapis.com`), `api_version` (default `v1beta`), `api_key`, and `timeout` (default 60 s). The API key travels in the `x-goog-api-key` header.',
    },
    {
      kind: 'list',
      items: [
        '**Streaming** — `stream_generate_content` POSTs to `:streamGenerateContent?alt=sse` and decodes the SSE chunks into a stream of `LlmResponse` values.',
        '**Server-side built-in tools** — when the request config carries `Tool::GoogleSearch {}`, `Tool::UrlContext {}`, or `Tool::CodeExecution {}` (injected by the [Gemini built-in tool handles](/docs/builtin-tools)), Gemini runs search grounding, URL grounding, or sandboxed Python on Google’s servers.',
        '**Context caching** — if the request carries a `ContextCacheConfig`, the client creates a server-side `cachedContents` entry for the stable prefix (system instruction + tools), reuses it on later calls keyed by a fingerprint, and transparently retries without the cache if the server rejects a stale entry. See [Context caching](/docs/context-caching).',
      ],
    },
    { kind: 'h2', text: 'Anthropic' },
    {
      kind: 'p',
      text: 'The Anthropic client targets the Messages API (`POST {base_url}/v1/messages`) with `x-api-key` and `anthropic-version` headers. `Anthropic::from_env(model_name)` reads `$ANTHROPIC_API_KEY`; `AnthropicConfig` exposes `base_url` (default `https://api.anthropic.com`), `anthropic_version` (default `2023-06-01`), `api_key`, and `timeout`.',
    },
    {
      kind: 'callout',
      tone: 'note',
      title: 'Streaming fallback',
      text: 'In the current release `Anthropic::stream_generate_content` performs a single-shot `generate_content` call and yields the result as a one-element stream — true SSE event accumulation is planned. The same applies to the OpenAI client. Gemini is the only provider with native chunked streaming today.',
    },
    { kind: 'h2', text: 'OpenAI (and Azure, Ollama, Groq)' },
    {
      kind: 'p',
      text: 'The `OpenAi` client speaks the `chat/completions` protocol, which makes it the bridge to every OpenAI-compatible endpoint. `OpenAi::from_env(model_name)` reads `$OPENAI_API_KEY` and honours `$OPENAI_BASE_URL` (default `https://api.openai.com/v1`). `OpenAiConfig` adds `api_version` (appended as Azure’s `?api-version=` query parameter) and `organization` (sent as the `OpenAI-Organization` header). The key travels as `Authorization: Bearer ...`.',
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
    { kind: 'h2', text: 'Transport security: HTTPS or loopback' },
    {
      kind: 'p',
      text: 'Every provider constructor validates its base URL with `transport_security::require_secure_url` before building the HTTP client. The rule: the destination must be `https://`, or a plaintext-HTTP **loopback** host (`localhost`, any `127.0.0.0/8` address, or `[::1]`). A public `http://` base URL is rejected with a configuration error rather than silently shipping your API key in cleartext. Loopback stays allowed so local mocks, Ollama, and test servers keep working.',
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
