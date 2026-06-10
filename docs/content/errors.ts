import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'errors',
  title: 'Error handling',
  description:
    'The crate-wide Error enum, its provider, tool, service, and schema sub-enums, and how failures surface in event streams.',
  srcPath: 'src/error.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Every public adk-rs API returns `adk_rs::Result<T>` — an alias for `Result<T, adk_rs::Error>`. The top-level `Error` enum wraps richer per-subsystem enums (provider, tool, service, schema) so call sites keep context while consumers match on a single type.',
    },
    { kind: 'h2', text: 'The Error enum' },
    {
      kind: 'table',
      head: ['Variant', 'Meaning'],
      rows: [
        ['`Provider(ProviderError)`', 'LLM provider client failure (transport, HTTP status, decoding, auth, rate limit, streaming).'],
        ['`Tool(ToolError)`', 'Tool invocation failure.'],
        ['`Service(ServiceError)`', 'Session / artifact / memory / credential backend failure.'],
        ['`Schema(SchemaError)`', 'Schema generation, sanitization, or validation failure.'],
        ['`Config(String)`', 'Invalid caller-supplied configuration — also what the [security guards](/docs/security) raise.'],
        ['`NotFound(String)`', 'The requested entity does not exist.'],
        ['`AlreadyExists(String)`', 'The entity already exists.'],
        ['`InvalidInput(String)`', 'Input validation failure (e.g. malformed args).'],
        ['`Io(std::io::Error)`', 'I/O failure (`#[from]`).'],
        ['`Json(serde_json::Error)`', 'JSON encode/decode failure (`#[from]`).'],
        ['`Other(String)`', 'Anything else, captured as a string.'],
      ],
    },
    {
      kind: 'p',
      text: 'Shorthand constructors exist for the string variants: `Error::config`, `Error::not_found`, `Error::already_exists`, `Error::invalid_input`, and `Error::other`. A small `Context` trait adds anyhow-style context: `result.context("loading eval set")?` wraps any displayable error into `Error::Other` with the message prefixed.',
    },
    { kind: 'h2', text: 'Sub-enums' },
    { kind: 'h3', text: 'ProviderError' },
    {
      kind: 'api',
      entries: [
        { sig: 'Transport(String)', desc: 'HTTP transport failure — DNS, TLS, connection reset.' },
        { sig: 'Http { status: u16, body: String }', desc: 'Non-2xx response from the provider; the body is preserved (truncated when very large).' },
        { sig: 'Decode(String)', desc: '2xx response whose body could not be decoded.' },
        { sig: 'Auth(String)', desc: 'Missing API key or bad credentials.' },
        { sig: 'RateLimit(String)', desc: 'Provider rate limit exceeded — the canonical retry-with-backoff signal.' },
        { sig: 'Stream(String)', desc: 'Streaming (SSE) protocol violation.' },
        { sig: 'Unsupported(&\'static str)', desc: 'The provider does not support a requested feature.' },
      ],
    },
    { kind: 'h3', text: 'ToolError' },
    {
      kind: 'api',
      entries: [
        { sig: 'InvalidArgs { tool: String, message: String }', desc: 'Arguments did not match the tool schema.' },
        { sig: 'Execution { tool: String, message: String }', desc: 'The tool failed while running.' },
        { sig: 'Aborted { tool: String, message: String }', desc: 'The tool requested aborting the entire agent turn.' },
        { sig: 'Unknown(String)', desc: 'No tool of this name is registered with the agent.' },
      ],
    },
    { kind: 'h3', text: 'ServiceError' },
    {
      kind: 'api',
      entries: [
        { sig: 'SessionNotFound(String)', desc: 'Session id unknown to the backend.' },
        { sig: 'ArtifactNotFound(String)', desc: 'Artifact key or version unknown.' },
        { sig: 'StaleSession(String)', desc: 'Optimistic-concurrency conflict: another writer updated the session.' },
        { sig: 'Backend(String)', desc: 'Database / storage failure (SQLite, Postgres, filesystem).' },
      ],
    },
    { kind: 'h3', text: 'SchemaError' },
    {
      kind: 'api',
      entries: [
        { sig: 'Sanitize(String)', desc: 'The schema could not be sanitized for the target provider.' },
        { sig: 'Invalid(String)', desc: 'The schema itself is invalid.' },
      ],
    },
    { kind: 'h2', text: 'How errors surface in event streams' },
    {
      kind: 'p',
      text: 'A run produces a stream of `Result<Event>`, and failures travel down it on three distinct channels:',
    },
    {
      kind: 'list',
      items: [
        '**Hard failures → `Err` items.** `LlmAgent` runs inside `try_stream!`, so a provider error from `generate_content` (or an exceeded LLM-call budget) becomes an `Err(Error)` item that terminates the stream. The [HTTP server](/docs/server) maps these to a 500 `{"detail": ...}` on `/run` or a final `data: {"error": ...}` SSE frame on `/run_sse`.',
        '**Soft model errors → error events.** When the model responds but the response is itself a failure — a blocking `finish_reason`, prompt feedback with a `block_reason`, or an empty candidate — the resulting `LlmResponse` carries `error_code` / `error_message` instead of content, and the event still flows as `Ok`. `LlmResponse::is_error()` is the check; on the wire these serialize as `errorCode` / `errorMessage`. The agent also synthesizes such events itself: `MAX_ITERATIONS` when the iteration budget is exhausted, and `CANCELLED` when the [cancellation token](/docs/cancellation-and-resume) trips.',
        '**Tool failures → error payloads.** A failed tool does not abort the turn: the agent converts the `Err` into a `{"error": "<message>"}` JSON function response and feeds it back to the model, which can apologise, retry, or pick another tool.',
      ],
    },
    { kind: 'h2', text: 'Matching on errors' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Retry-aware stream consumption',
      code: `use adk_rs::error::{Error, ProviderError};
use futures::StreamExt;

while let Some(item) = events.next().await {
    match item {
        Ok(event) => {
            if event.response.is_error() {
                eprintln!(
                    "model-level error: {:?} {:?}",
                    event.response.error_code, event.response.error_message
                );
                continue;
            }
            handle(event);
        }
        Err(Error::Provider(ProviderError::RateLimit(msg))) => {
            // Back off and retry the turn.
            eprintln!("rate limited: {msg}");
            break;
        }
        Err(Error::Provider(ProviderError::Http { status, .. })) if status >= 500 => {
            // Transient server error — also retryable.
            break;
        }
        Err(e) => return Err(e),
    }
}`,
    },
    {
      kind: 'p',
      text: 'Note that cancellation never surfaces as an `Err` item: a cancelled run yields a final `Ok` event whose `error_code` is `Some("CANCELLED")`, caught by the `is_error()` branch above.',
    },
    {
      kind: 'callout',
      tone: 'tip',
      title: 'Which variants are retry-worthy',
      text: 'Treat `ProviderError::RateLimit`, `ProviderError::Transport`, and `ProviderError::Http` with a 5xx (or 429) status as transient and retryable with backoff. `Auth`, `Decode`, `Unsupported`, and every `Config` / `InvalidInput` error are deterministic — retrying will not help.',
    },
    { kind: 'h2', text: 'Error-recovery callbacks' },
    {
      kind: 'p',
      text: '`adk_rs::core` exports two error-hook callback type aliases: `OnModelErrorCallback` — `Fn(&mut CallbackContext, &mut LlmRequest, &Error) -> Future<Result<Option<LlmResponse>>>` — and `OnToolErrorCallback` — `Fn(&mut ToolContext, &Arc<dyn DynTool>, &Value, &Error) -> Future<Result<Option<Value>>>`. Per their contract, returning `Ok(Some(...))` substitutes a recovery value for the failed call. Register them via `LlmAgentBuilder::on_model_error_callback` and `::on_tool_error_callback` — a recovered model error never becomes an `Err` stream item; the substitute response continues the turn as if the call had succeeded. See [Callbacks & plugins](/docs/callbacks-and-plugins) for the callback system and how hooks attach to the agent lifecycle; for run-level observation of failures, `BasePlugin::after_run` receives the run’s failure as `Option<&Error>`.',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Events](/docs/events) — the `Event` / `LlmResponse` shapes that carry soft errors.',
        '[Callbacks & plugins](/docs/callbacks-and-plugins) — lifecycle hooks, including the error callbacks.',
        '[Providers](/docs/providers) — where `ProviderError`s originate.',
      ],
    },
  ],
};
