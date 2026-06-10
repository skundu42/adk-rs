import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'server',
  title: 'HTTP server',
  description:
    'Expose runners over HTTP and SSE with an axum server that works with the adk-web dev UI out of the box.',
  srcPath: 'src/server/app.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'The `server` feature exposes one or more Runners over HTTP and Server-Sent Events. The endpoint surface implements the wire contract the adk-web Angular UI expects — camelCase JSON, `{"detail": ...}` errors, `data: <json>` SSE framing — so existing ADK web frontends and API clients talk to an adk-rs server unchanged.',
    },
    {
      kind: 'p',
      text: 'Three pieces make up the server: `AppState` (the runners plus auth and CORS settings), `build_router` (the axum `Router`), and `serve` / `serve_with` (bind-and-listen with a loopback safety guard). All live in `adk_rs::server`.',
    },
    { kind: 'h2', text: 'AppState' },
    {
      kind: 'p',
      text: '`AppState` carries a map of agent name → `Arc<Runner>`, an optional bearer token, and the CORS allow-list. Apps are addressed by `Runner::app_name()` on the wire, so the map key only matters for the legacy `/list-agents` route.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn unauthenticated(runners: Arc<HashMap<String, Arc<Runner>>>) -> AppState',
          desc: 'No authentication. `serve` will then refuse non-loopback binds (see below).',
        },
        {
          sig: 'fn with_bearer_token(runners: Arc<HashMap<String, Arc<Runner>>>, token: impl Into<String>) -> AppState',
          desc: 'Require `Authorization: Bearer <token>` on every request. Comparison is constant-time.',
        },
        {
          sig: 'fn with_allow_origins(self, origins: impl IntoIterator<Item = String>) -> Self',
          desc: 'Emit CORS headers for the given origins (use `"*"` for any). Needed when the adk-web UI runs on a different origin, e.g. `http://localhost:4200`. Empty (the default) emits no CORS headers.',
        },
      ],
    },
    { kind: 'h2', text: 'build_router, serve, serve_with' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn build_router(state: AppState) -> Router',
          desc: 'Build the axum router. When `state.auth_token` is set, every route is wrapped in the bearer check. Useful for embedding into a larger app or driving with `tower::ServiceExt::oneshot` in tests.',
        },
        {
          sig: 'async fn serve(addr: SocketAddr, state: AppState) -> Result<()>',
          desc: 'Bind and serve with `ServeOptions::default()`.',
        },
        {
          sig: 'async fn serve_with(addr: SocketAddr, state: AppState, opts: ServeOptions) -> Result<()>',
          desc: 'Like `serve` but with explicit options. Refuses a non-loopback bind that has no auth token unless `opts.dangerously_allow_unauthenticated_remote` is `true`, and always logs a warning when listening on a non-loopback interface.',
        },
      ],
    },
    {
      kind: 'callout',
      tone: 'warn',
      title: 'Loopback by default',
      text: '`ServeOptions` has a single field, `dangerously_allow_unauthenticated_remote: bool` (default `false`). Without it, binding `0.0.0.0` with no bearer token returns a config error — otherwise anyone reachable on the network could drive your agents and read every session’s history. See [Security model](/docs/security).',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Serving two agents',
      code: `use adk_rs::server::{AppState, ServeOptions, serve_with};
use std::collections::HashMap;
use std::sync::Arc;

let mut runners = HashMap::new();
runners.insert("greeter".to_string(), Arc::new(greeter_runner));
runners.insert("researcher".to_string(), Arc::new(research_runner));

let state = AppState::with_bearer_token(Arc::new(runners), std::env::var("ADK_WEB_TOKEN")?)
    .with_allow_origins(["http://localhost:4200".to_string()]);

serve_with("0.0.0.0:8000".parse()?, state, ServeOptions::default()).await?;`,
    },
    { kind: 'h2', text: 'Endpoint reference' },
    {
      kind: 'table',
      head: ['Method', 'Path', 'Purpose'],
      rows: [
        ['GET', '`/health`', 'Liveness: `{"status": "ok"}`.'],
        ['GET', '`/version`', 'Crate version plus `"language": "rust"`.'],
        ['GET', '`/list-apps`', 'Bare JSON array of registered app names (sorted, deduplicated).'],
        ['GET', '`/list-agents`', 'Legacy adk-rs route: `{"agents": [...]}` keyed by registration name.'],
        ['POST', '`/run`', 'Run one turn to completion; returns a bare JSON array of wire events.'],
        ['POST', '`/run_sse`', 'Same body as `/run`, but events stream as SSE `data:` frames.'],
        ['GET', '`/apps/:app/users/:user/sessions`', 'List sessions (array of session objects without state/events).'],
        ['POST', '`/apps/:app/users/:user/sessions`', 'Create a session; body may carry `sessionId`, `state`, and `events`. Duplicate explicit id → 409.'],
        ['GET', '`/apps/:app/users/:user/sessions/:session`', 'Fetch one session including its full event log. Missing → 404.'],
        ['POST', '`/apps/:app/users/:user/sessions/:session`', 'Create a session with this explicit id; body is an optional initial state map.'],
        ['PATCH', '`/apps/:app/users/:user/sessions/:session`', 'Apply a `stateDelta` via a synthetic user event; returns the updated session.'],
        ['DELETE', '`/apps/:app/users/:user/sessions/:session`', 'Delete the session; returns `200` with a `null` body.'],
        ['GET', '`.../sessions/:session/artifacts`', 'List artifact names for the session.'],
        ['GET', '`.../sessions/:session/artifacts/:name`', 'Load the latest artifact version (or `?version=N`) as a `Part` JSON object.'],
        ['DELETE', '`.../sessions/:session/artifacts/:name`', 'Delete an artifact; returns `null`.'],
        ['GET', '`.../sessions/:session/artifacts/:name/versions`', 'List version numbers.'],
        ['GET', '`.../sessions/:session/artifacts/:name/versions/:version`', 'Load a specific version.'],
        ['PATCH', '`/apps/:app/users/:user/memory`', 'Body `{"sessionId": ...}` — add that session to the configured [memory service](/docs/memory).'],
        ['GET', '`/debug/trace/:event_id`', 'Trace stub: always `404 {"detail": "Trace not found"}`.'],
        ['GET', '`/debug/trace/session/:session`', 'Trace stub: always `[]` so the UI trace tab degrades gracefully.'],
        ['GET', '`/dev/apps/:app/debug/trace/:event_id`', '2.x-path alias of the trace stub.'],
        ['GET', '`/dev/apps/:app/debug/trace/session/:session`', '2.x-path alias; returns `[]`.'],
        ['GET', '`/apps/:app/eval_sets`, `/apps/:app/eval_results`', 'Eval stubs: `[]` (run evals via the [CLI](/docs/cli) instead).'],
        ['GET', '`/dev/apps/:app/eval_sets`, `/dev/apps/:app/eval_results`', '2.x-path aliases; return `[]`.'],
        ['GET', '`/dev/apps/:app/metrics-info`', 'Returns `{"metricsInfo": []}`.'],
      ],
    },
    {
      kind: 'p',
      text: 'Artifact routes return `404 {"detail": "Artifact service is not configured"}` when the runner has no [artifact service](/docs/artifacts); the memory route returns `400` without a memory service. Both snake_case and camelCase request fields are accepted (`sessionId` / `session_id`), mirroring pydantic’s `populate_by_name`.',
    },
    { kind: 'h2', text: 'Wire format' },
    {
      kind: 'p',
      text: 'Responses use the wire dialect implemented in `src/server/wire.rs`: events flatten the `LlmResponse` fields (`content`, `finishReason`, `errorCode`, `errorMessage`, `usageMetadata`, `cacheMetadata`, …) next to event-level fields, `null` fields are omitted, and `id` / `invocationId` / `author` / `timestamp` / `actions` are always present. Inside `actions`, four maps are always emitted — `stateDelta`, `artifactDelta`, `requestedAuthConfigs`, `requestedToolConfirmations` — because the UI reads them unconditionally. Errors take the shape `{"detail": "Session not found"}` with the matching HTTP status.',
    },
    {
      kind: 'p',
      text: 'On `/run_sse` every frame is `data: <one-line JSON>` followed by a blank line. An event that carries both content parts and a non-empty `artifactDelta` is split into two frames (content first, then the delta) — a deliberate framing quirk the adk-web UI depends on. A mid-stream failure emits a final `data: {"error": "..."}` frame and closes.',
    },
    { kind: 'h2', text: 'Calling /run and /run_sse' },
    {
      kind: 'code',
      lang: 'bash',
      title: 'curl: create a session, then run',
      code: `# Create the session first — /run does not auto-create.
curl -X POST http://127.0.0.1:8000/apps/test-app/users/alice/sessions \\
  -H 'content-type: application/json' \\
  -d '{"sessionId": "s-1"}'

curl -X POST http://127.0.0.1:8000/run \\
  -H 'content-type: application/json' \\
  -d '{
    "appName": "test-app",
    "userId": "alice",
    "sessionId": "s-1",
    "newMessage": {"role": "user", "parts": [{"text": "hello"}]},
    "streaming": false
  }'
# -> [{"content":{"parts":[{"text":"..."}],"role":"model"},"author":"greet",
#      "invocationId":"...","timestamp":...,"actions":{"stateDelta":{},...}}]`,
    },
    {
      kind: 'code',
      lang: 'bash',
      title: 'curl: streaming over SSE',
      code: `curl -N -X POST http://127.0.0.1:8000/run_sse \\
  -H 'content-type: application/json' \\
  -d '{
    "appName": "test-app",
    "userId": "alice",
    "sessionId": "s-1",
    "newMessage": {"role": "user", "parts": [{"text": "hello"}]},
    "streaming": true
  }'
# data: {"content":{"parts":[{"text":"..."}],"role":"model"},...}`,
    },
    {
      kind: 'p',
      text: 'The request body carries: `appName` (optional when exactly one app is registered), `userId`, `sessionId`, `newMessage`, `streaming` (`true` selects `StreamingMode::Sse` partial events), optional `stateDelta` applied before the run, and optional `invocationId` to [resume a paused invocation](/docs/cancellation-and-resume) instead of starting a new one.',
    },
    { kind: 'h2', text: 'Connecting the adk-web UI' },
    {
      kind: 'list',
      items: [
        'Run the adk-web Angular dev UI and point its `runtime-config.json` `backendUrl` at this server.',
        'Pass the UI’s origin to `AppState::with_allow_origins(["http://localhost:4200".into()])` so CORS preflights succeed.',
        'The trace and eval tabs degrade gracefully against the stub routes; everything else (chat, sessions, state, artifacts) works end to end.',
      ],
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Embedded CLI](/docs/cli) — `my-app web` starts this server with one flag.',
        '[Security model](/docs/security) — bind policy and bearer-token details.',
        '[Events](/docs/events) — the native event shape that the wire format serializes.',
      ],
    },
  ],
};
