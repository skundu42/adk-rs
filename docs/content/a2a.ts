import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'a2a',
  title: 'A2A protocol',
  description:
    'Expose a Runner as a Google Agent-to-Agent JSON-RPC endpoint and call remote A2A agents as local BaseAgents.',
  srcPath: 'src/a2a/mod.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'A2A is Google’s Agent-to-Agent protocol: JSON-RPC 2.0 over HTTP with SSE streaming, agent-card discovery, and a task lifecycle. The `a2a` feature ships both halves — a server bridge that exposes any local `Runner` as an A2A endpoint, and `RemoteA2aAgent`, a [`BaseAgent`](/docs/agents-overview) that proxies to a remote A2A server.',
    },
    { kind: 'h2', text: 'Serving a Runner over A2A' },
    {
      kind: 'p',
      text: 'The server side is configured with an `AgentCard` (what the agent advertises), an `A2aServerConfig` (paths and auth), and an `A2aState` (the runner plus task persistence and webhook delivery).',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Expose a local Runner',
      code: `use adk_rs::a2a::{
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
serve("127.0.0.1:8080".parse()?, state).await?;`,
    },
    {
      kind: 'p',
      text: 'The `AgentCard` carries `name`, `description`, `url` (the JSON-RPC endpoint), `version`, optional `provider` / `documentation_url` / `authentication`, `capabilities` (`streaming`, `push_notifications`, `state_transition_history`), `default_input_modes` / `default_output_modes` (MIME types), and a list of `AgentSkill`s. It is served at `/.well-known/agent.json` by default — the path is configurable via `A2aServerConfig::agent_card_path`, and the JSON-RPC mount path via `rpc_path` (default `/`). `A2aState` forces `capabilities.push_notifications = true` on the served card, because the bridge always supports webhooks.',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'fn A2aServerConfig::new(agent_card: AgentCard) -> Self', desc: 'Defaults: card at `/.well-known/agent.json`, RPC at `/`, no auth.' },
        { sig: 'fn with_bearer_token(self, token: impl Into<String>) -> Self', desc: 'Require `Authorization: Bearer <token>` on every request (constant-time comparison).' },
        { sig: 'fn A2aState::new(runner: Arc<Runner>, cfg: A2aServerConfig) -> Self', desc: 'Uses the default `InMemoryTaskService` for task persistence.' },
        { sig: 'fn A2aState::with_task_service(runner, cfg, tasks: Arc<dyn TaskService>) -> Self', desc: 'Plug in your own Redis / SQL / Firestore task backend.' },
        { sig: 'fn router(state: A2aState) -> axum::Router', desc: 'Build the router for embedding into an existing axum app.' },
        { sig: 'async fn serve(addr: SocketAddr, state: A2aState) -> Result<()>', desc: 'Bind and serve. Refuses non-loopback binds without a bearer token.' },
        { sig: 'async fn serve_with(addr, state, opts: ServeOptions) -> Result<()>', desc: '`ServeOptions { dangerously_allow_unauthenticated_remote }` opts out of the bind guard, mirroring the [HTTP server](/docs/server).' },
      ],
    },
    { kind: 'h2', text: 'JSON-RPC methods' },
    {
      kind: 'table',
      head: ['Method', 'Behaviour'],
      rows: [
        ['`message/send`', 'Synchronous: create a task, run the agent to completion, return the final `Task` with accumulated `history` and `artifacts`.'],
        ['`message/stream`', 'Same input, but the response is an SSE channel: an initial `Task` snapshot (so the caller has the id), then every `TaskStatusUpdateEvent` / `TaskArtifactUpdateEvent`, closed by a `done` SSE event.'],
        ['`tasks/get`', 'Look up a `Task` by id; `historyLength` trims the returned history.'],
        ['`tasks/cancel`', 'Flip the runner’s cancellation token for the task’s invocation, then mark the task `canceled`. Terminal tasks return `-32002`.'],
        ['`tasks/resubscribe`', 'Re-attach to a task’s SSE channel: a current `Task` snapshot, then live updates until a final status.'],
        ['`tasks/pushNotificationConfig/set`', 'Register a webhook for a task. Plaintext public `http://` URLs are refused with `-32602` (HTTPS-or-loopback rule).'],
        ['`tasks/pushNotificationConfig/get`', 'Retrieve a config by `pushNotificationConfigId` (or the first one when omitted).'],
        ['`tasks/pushNotificationConfig/list`', 'Enumerate every config registered for a task.'],
        ['`tasks/pushNotificationConfig/delete`', 'Remove one config — or all of them when the id is omitted. Returns `{"removed": n}`.'],
      ],
    },
    {
      kind: 'p',
      text: 'Errors use the JSON-RPC reserved range plus two A2A-specific codes: `-32001` (`TASK_NOT_FOUND`) and `-32002` (`TASK_NOT_CANCELABLE`). If an agent pauses awaiting user input — a [tool confirmation](/docs/tool-confirmation), an [auth consent](/docs/auth), or a long-running tool result — the task ends in `input-required` rather than `completed`.',
    },
    { kind: 'h2', text: 'Task lifecycle and TaskService' },
    {
      kind: 'p',
      text: '`TaskState` is a closed set: `submitted`, `working`, `input-required`, `completed`, `canceled`, `failed`, `rejected`, `auth-required`, `unknown` — with `is_terminal()` true for `completed` / `canceled` / `failed` / `rejected`. A `message/*` call creates the task as `submitted`, transitions it to `working` before the agent starts, and finishes with `completed`, `input-required`, or `failed`.',
    },
    {
      kind: 'p',
      text: 'Persistence is the `TaskService` trait: `create_task`, `get_task`, `update_status` (which broadcasts a `TaskStatusUpdateEvent` to all SSE subscribers), `append_history`, `append_artifact`, `subscribe`, `cancel_task`, and the four `*_push_config` methods. The shipped `InMemoryTaskService` stores tasks behind a mutex and fans updates out via per-task `tokio::sync::broadcast` channels; implement the trait directly for an external store.',
    },
    { kind: 'h2', text: 'Push notifications' },
    {
      kind: 'p',
      text: '`PushNotifier` (one per `A2aState`) delivers updates out-of-band: for each registered config it spawns a subscriber that POSTs every status / artifact update to the webhook URL. Delivery is best-effort and fire-and-forget — failures are logged, retried up to 3 times with a small backoff, and never surfaced to the inbound caller. Webhook bodies match the `message/stream` envelope (`{"jsonrpc": "2.0", "result": <update>}`), so a receiver written for SSE consumes them unchanged. A configured `token` is sent as `Authorization: Bearer <token>` on each POST, and webhook URLs must be HTTPS or loopback.',
    },
    { kind: 'h2', text: 'Calling a remote agent' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'RemoteA2aAgent as a local BaseAgent',
      code: `use adk_rs::a2a::{RemoteA2aAgent, RemoteA2aConfig};
use std::time::Duration;

let remote = RemoteA2aAgent::connect(RemoteA2aConfig {
    name: "fallback".into(), // overridden by the fetched agent card
    url: "https://peer.example.com/a2a/".into(),
    agent_card_url: Some("https://peer.example.com/.well-known/agent.json".into()),
    stream: true, // use message/stream over SSE
    timeout: Duration::from_secs(60),
    ..Default::default()
})
.await?;`,
    },
    {
      kind: 'p',
      text: '`RemoteA2aConfig` has seven fields: `name` and `description` (local fallbacks), `url` (the JSON-RPC endpoint), `agent_card_url` (when `Some`, `connect` fetches the card and adopts its name and description; `RemoteA2aAgent::new` skips discovery entirely), `headers` (extra HTTP headers such as `Authorization`), `timeout` (default 120 s), and `stream` (use `message/stream` SSE instead of synchronous `message/send`; default `false`).',
    },
    {
      kind: 'p',
      text: 'Because `RemoteA2aAgent` implements `BaseAgent`, it slots into agent trees exactly like a local `LlmAgent` — `sub_agent(remote)`, `AgentTool::new(remote)`, or as a step in a [workflow agent](/docs/workflow-agents). On each turn it converts the invocation’s user content into an A2A `Message` (carrying the session id as `contextId` and the user id in metadata), dispatches it, and converts the resulting `Task` history or streamed updates back into ADK [events](/docs/events). `message_send` and `message_stream` are also public for direct protocol use.',
    },
    {
      kind: 'callout',
      tone: 'warn',
      title: 'Credentials need HTTPS',
      text: 'If `headers` contains a credential-bearing header (`Authorization`, `Cookie`, `Proxy-Authorization`, or anything starting with `x-api` / `x-auth`), construction refuses non-loopback `http://` URLs for both the RPC endpoint and the agent-card URL. See [Security model](/docs/security).',
    },
    { kind: 'h2', text: 'Cancellation routes back to the runner' },
    {
      kind: 'p',
      text: 'When the bridge starts a task, it records the runner’s `invocation_id` in the task’s metadata under `adk:invocationId`. A later `tasks/cancel` looks that id up and calls `Runner::cancel` first, so the in-flight agent observes the [cooperative cancellation token](/docs/cancellation-and-resume) and stops cleanly before the task is marked `canceled`.',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Runner](/docs/runner) — `start`, `cancel`, and the invocation registry the bridge drives.',
        '[Cancellation & resume](/docs/cancellation-and-resume) — what happens inside the agent on cancel.',
        '[Security model](/docs/security) — the HTTPS-or-loopback and bind guards A2A shares with the rest of the crate.',
      ],
    },
  ],
};
