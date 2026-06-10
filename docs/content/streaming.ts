import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'streaming',
  title: 'Streaming',
  description:
    'StreamingMode, partial events, the LlmResponseStream provider contract, and how SSE flows out through the HTTP server.',
  srcPath: 'src/core/run_config.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs streams at two layers: every run is already an incremental stream of typed events, and StreamingMode::Sse additionally requests token-by-token deltas from the model. The partial / turn_complete fields on Event define the contract for rendering them.',
    },
    { kind: 'h2', text: 'StreamingMode' },
    {
      kind: 'p',
      text: '`StreamingMode` lives in `adk_rs::core::run_config` (re-exported as `adk_rs::core::StreamingMode`) and is carried on [`RunConfig`](/docs/runner):',
    },
    {
      kind: 'table',
      head: ['Variant', 'Meaning'],
      rows: [
        ['`StreamingMode::None`', 'Default. One final response per model turn — no partials.'],
        ['`StreamingMode::Sse`', 'Server-sent events: token-by-token deltas terminated by a final event.'],
      ],
    },
    {
      kind: 'p',
      text: '`Runner::run` always uses `RunConfig::default()` (mode `None`). To request streaming, build the `RunConfig` yourself and go through `run_with` (or `start`):',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Enabling SSE streaming via run_with',
      code: `use adk_rs::core::{RunConfig, StreamingMode};
use adk_rs::genai_types::Content;
use futures::StreamExt;

let run_config = RunConfig {
    streaming_mode: StreamingMode::Sse,
    ..RunConfig::default()
};
let mut events = runner
    .run_with("alice", Some("s1"), Content::user_text("Tell me a story"), run_config)
    .await?;

let mut rendered = String::new();
while let Some(event) = events.next().await {
    let event = event?;
    let Some(content) = &event.response.content else { continue };
    if event.partial == Some(true) {
        // Transient chunk: append the delta and repaint.
        rendered.push_str(&content.text_concat());
        print!("\\r{rendered}");
    } else {
        // Authoritative event: replaces anything rendered from partials.
        rendered = content.text_concat();
        println!("\\r{rendered}");
    }
    if event.turn_complete == Some(true) {
        rendered.clear();
    }
}`,
    },
    { kind: 'h2', text: 'Partial events and turn_complete' },
    {
      kind: 'p',
      text: 'The [`Event`](/docs/events) contract for streaming is two optional flags. `partial: Some(true)` marks a transient chunk: the session layer refuses to persist it (`apply_event_to_session` and `append_event_locked` both return early for partials), and the runner only mirrors **non-partial** events into the session store. `turn_complete: Some(true)` marks the event that ends a model turn — in the bundled `LlmAgent` every model-response event is stamped with it.',
    },
    {
      kind: 'list',
      items: [
        'Render text from `partial == Some(true)` events incrementally, but treat it as provisional.',
        'When a non-partial event for the same turn arrives, it is the authoritative, persisted version — replace anything you rendered from partials.',
        'Use `turn_complete == Some(true)` (or `Event::is_final_response()`) to know when to finalize a message bubble.',
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'In v0.3.0 the `LlmAgent` turn loop issues one `generate_content` call per turn and emits whole-turn events, so the event stream is incremental at event granularity (each model turn, tool call, and tool result is yielded as soon as it happens). Token-level provider streams are available directly through `Model::stream_generate_content`; the `partial`/`turn_complete` fields define how such chunks are represented as events.',
    },
    { kind: 'h2', text: 'LlmResponseStream and the Model trait' },
    {
      kind: 'p',
      text: 'At the provider layer, streaming is the `Model::stream_generate_content` method. It returns an `LlmResponseStream`, defined in `adk_rs::core::stream` as `Pin<Box<dyn Stream<Item = Result<LlmResponse>> + Send>>`.',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'async fn generate_content(&self, req: LlmRequest) -> Result<LlmResponse>', desc: 'Single-shot generation. Required for every `Model` implementation.' },
        { sig: 'async fn stream_generate_content(&self, req: LlmRequest) -> Result<LlmResponseStream>', desc: 'Streaming generation. The **default implementation** falls back to `generate_content` and wraps the single response in a one-element stream — so every provider is stream-callable, even ones with no native streaming.' },
      ],
    },
    {
      kind: 'p',
      text: 'The Gemini provider overrides the default: `src/providers/gemini/stream.rs` converts the `reqwest` SSE body into an `LlmResponseStream` with `eventsource_stream`, parsing each `data:` frame into an `LlmResponse` chunk. The Anthropic and OpenAI-compatible providers implement `stream_generate_content` as well.',
    },
    { kind: 'h2', text: 'SSE over the HTTP server' },
    {
      kind: 'p',
      text: 'With the `server` feature, the dev server exposes `POST /run_sse` next to the buffered `POST /run` (see [Server](/docs/server)). The handler maps the request’s `streaming` flag to `StreamingMode::Sse` in the `RunConfig`, starts the invocation, and forwards the runner’s event stream as it arrives — each `Event` becomes one `data:` frame (events that carry both content parts and an `artifactDelta` are split into two frames), with errors emitted as a final `{ "error": ... }` frame. `POST /run` consumes the same stream but buffers it into a JSON array.',
    },
    {
      kind: 'code',
      lang: 'bash',
      title: 'Streaming a turn from the dev server',
      code: `curl -N -X POST http://127.0.0.1:8000/run_sse \\
  -H 'Content-Type: application/json' \\
  -d '{
    "appName": "demo",
    "userId": "alice",
    "sessionId": "s1",
    "newMessage": {"role": "user", "parts": [{"text": "hi"}]},
    "streaming": true
  }'`,
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[The Runner](/docs/runner) — `run_with`, `start`, and `RunConfig`.',
        '[Events](/docs/events) — the full `Event` and `EventActions` reference.',
        '[Server](/docs/server) — every HTTP endpoint, including `/run_sse`.',
        '[Models](/docs/models) — the `Model` trait and providers.',
      ],
    },
  ],
};
