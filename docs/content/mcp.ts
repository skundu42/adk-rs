import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'mcp',
  title: 'MCP integration',
  description:
    'Connecting agents to Model Context Protocol servers over stdio or streamable HTTP with McpToolset, including confirmation policies and transport security.',
  srcPath: 'src/mcp/toolset.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'The `mcp` feature turns any Model Context Protocol server into an adk-rs toolset: `McpToolset` connects, performs the MCP `initialize` handshake, lists the server’s tools, and exposes each one as a `DynTool` whose `run` proxies to `tools/call`. Two transports are supported — spawn-a-child-process **stdio** and remote **streamable HTTP**.',
    },
    { kind: 'h2', text: 'McpToolset' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'McpToolset::stdio(params: McpStdioParams) -> Result<Self>',
          desc: 'Spawn an MCP server as a child process and talk newline-delimited JSON-RPC over stdin/stdout.',
        },
        {
          sig: 'McpToolset::http(params: McpHttpParams) -> Result<Self>',
          desc: 'Connect to a remote MCP server over the streamable-HTTP profile.',
        },
        {
          sig: 'McpToolset::from_client(client: Arc<McpClient>) -> Self',
          desc: 'Wrap an already-connected `McpClient` (e.g. one you built from a custom `Transport`).',
        },
        {
          sig: 'fn with_confirmation_policy(self, policy: ConfirmationPolicy) -> Self',
          desc: 'Mark some or all discovered tools as requiring user confirmation. Must be set **before** the first `list_tools` call — the discovered tool list is cached.',
        },
        {
          sig: 'async fn list_tools(&self, ctx: &ReadonlyContext) -> Result<Vec<Arc<dyn DynTool>>>',
          desc: 'The `Toolset` impl: fetches `tools/list` from the server once, converts each descriptor into an `McpTool`, and caches the result for subsequent calls.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'Each discovered tool keeps the server’s name and description; its `inputSchema` is parsed into an adk `Schema` for the declaration (falling back to an empty object schema, with a warning, if it doesn’t parse). Calling the tool sends `tools/call` with `{"name", "arguments"}` and returns the server’s result JSON unchanged.',
    },
    { kind: 'h2', text: 'Stdio transport' },
    {
      kind: 'p',
      text: '`McpStdioParams` has four fields: `command: String`, `args: Vec<String>`, `env: HashMap<String, String>` (extra environment variables), and `timeout: Duration` (per call, default 30 s). The child is spawned with piped stdio and `kill_on_drop`; its stderr lines are forwarded to `tracing` warnings so server logs surface in yours. An empty `command` is rejected up front.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Filesystem MCP server over stdio',
      code: `use adk_rs::mcp::{McpStdioParams, McpToolset};
use std::time::Duration;

let toolset = McpToolset::stdio(McpStdioParams {
    command: "npx".into(),
    args: vec![
        "-y".into(),
        "@modelcontextprotocol/server-filesystem".into(),
        "/tmp/sandbox".into(),
    ],
    env: std::collections::HashMap::new(),
    timeout: Duration::from_secs(30),
})
.await?;`,
    },
    { kind: 'h2', text: 'Streamable HTTP transport' },
    {
      kind: 'p',
      text: '`McpHttpParams` has three fields: `url: String` (the single MCP endpoint), `headers: HashMap<String, String>` (e.g. `Authorization: Bearer ...`), and `timeout: Duration` (default 60 s). The transport follows the MCP streamable-HTTP profile: every JSON-RPC request is a POST to that one endpoint with `Accept: application/json, text/event-stream`, and the server answers either with a plain `application/json` body or with a `text/event-stream` whose events carry JSON-RPC responses — the client scans the stream for the envelope matching its request id, logging and skipping interleaved notifications.',
    },
    {
      kind: 'list',
      items: [
        '**Session affinity** — a server-issued `Mcp-Session-Id` response header is remembered and echoed on every subsequent request.',
        '**Legacy SSE servers** — the older two-endpoint pattern (GET for the event channel, separate POST for messages) is **not** supported; the streamable-HTTP profile subsumed it in the MCP 2025-03 spec.',
        '**Handshake** — `McpClient` drives `initialize` (advertising protocol version `2024-11-05` and tool capability) followed by the `notifications/initialized` notification before the toolset is returned.',
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Remote MCP server over HTTP',
      code: `use adk_rs::mcp::{McpHttpParams, McpToolset};
use std::collections::HashMap;
use std::time::Duration;

let mut headers = HashMap::new();
headers.insert("Authorization".into(), format!("Bearer {token}"));

let toolset = McpToolset::http(McpHttpParams {
    url: "https://mcp.example.com/mcp".into(),
    headers,
    timeout: Duration::from_secs(60),
})
.await?;`,
    },
    { kind: 'h2', text: 'Confirmation policy (human-in-the-loop)' },
    {
      kind: 'p',
      text: 'MCP tools are remote code you didn’t write, so the toolset supports gating them behind [tool confirmation](/docs/tool-confirmation). `ConfirmationPolicy` has three variants: `None` (default — nothing gated), `All` (every discovered tool pauses for approval), and `Named(HashSet<String>)` (only the listed tool names). Gated tools report `requires_confirmation() == true`, so the agent pauses with an `adk_request_confirmation` request instead of dispatching.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Confirm only destructive tools',
      code: `use adk_rs::mcp::{ConfirmationPolicy, McpHttpParams, McpToolset};
use std::collections::HashSet;

let gated: HashSet<String> = ["delete_file", "move_file"]
    .into_iter()
    .map(String::from)
    .collect();

let toolset = McpToolset::http(params)
    .await?
    .with_confirmation_policy(ConfirmationPolicy::Named(gated));`,
    },
    { kind: 'h2', text: 'Credentials never travel in plaintext' },
    {
      kind: 'p',
      text: 'When `McpHttpParams.headers` contains a credential-bearing header — `Authorization`, `Cookie`, `Proxy-Authorization`, or anything starting with `x-api` / `x-auth` (case-insensitive) — the transport requires the URL to be `https://` or a loopback host, refusing construction otherwise. Unauthenticated plaintext HTTP remains allowed, and authenticated loopback (`http://127.0.0.1:...`) works for local development. The transport also disables redirects, so credential-bearing headers can’t be re-sent to a redirect target.',
    },
    { kind: 'h2', text: 'Lower-level access: McpClient' },
    {
      kind: 'p',
      text: 'If you need raw protocol access, `McpClient` exposes `connect(transport)`, `spawn(McpStdioParams)`, `http(McpHttpParams)`, generic `call(method, params)` / `notify(method, params)`, plus typed `list_tools() -> Vec<McpToolDescriptor>` and `call_tool(name, args)`. The `Transport` trait (`call` + `notify`) is public too, so a custom transport — say, WebSocket — can plug into the same client and toolset.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Tools overview](/docs/tools-overview) — how toolset tools join the agent’s dispatch loop.',
        '[Tool confirmation](/docs/tool-confirmation) — the pause/approve/resume cycle in detail.',
        '[Security](/docs/security) — the plaintext-HTTP guard and the rest of the threat model.',
      ],
    },
  ],
};
