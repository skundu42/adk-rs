import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'security',
  title: 'Security model',
  description:
    'The four guard families that make adk-rs secure by default, what trips them, and how to opt out deliberately.',
  srcPath: 'src/transport_security.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'adk-rs is secure by default: a handful of guards trip whenever behaviour would be unsafe — sending credentials over plaintext HTTP, exposing an unauthenticated control plane to the network, writing attacker-controlled paths, or running model-generated code with host privileges. Each guard has exactly one deliberate opt-out, and the crate as a whole is `#![forbid(unsafe_code)]`.',
    },
    { kind: 'h2', text: '1. HTTPS-only credentials' },
    {
      kind: 'p',
      text: 'Every credential-bearing outbound client routes its destination through `transport_security::require_secure_url(url, field)`. The rule: the URL must be `https://` (case-insensitive) or a loopback `http://` host. Anything else is rejected at construction time, before any request is built.',
    },
    {
      kind: 'list',
      items: [
        '**Who checks**: the provider clients (`Gemini`, `Anthropic`, `OpenAi`), `RestApiTool` ([OpenAPI tools](/docs/openapi-tools)), the [MCP](/docs/mcp) HTTP transport, the [A2A client](/docs/a2a) when its extra headers look credential-bearing (`Authorization`, `Cookie`, `Proxy-Authorization`, `x-api*`, `x-auth*`), and A2A push-notification webhook URLs.',
        '**Loopback exemption**: `localhost`, any `127.0.0.0/8` address, and `[::1]` pass — this is what lets tests and local mocks (e.g. wiremock, Ollama) work over plain HTTP.',
        '**No smuggling**: the check strips userinfo, IPv6 brackets, and ports before classifying the host, so `http://127.0.0.1@evil.example.com` is still rejected. Unknown schemes (`ftp://`, `file://`, scheme-less strings) always fail.',
        '**Secret-safe errors**: the error message names the offending *field* (e.g. `OpenAiConfig.base_url`), never the URL itself, because URLs can carry secrets in their userinfo.',
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      title: 'Opting out',
      text: 'There is no flag to disable this guard. The supported escape hatch is the loopback exemption: terminate TLS in front of the plain-HTTP service, or proxy it via localhost.',
    },
    { kind: 'h2', text: '2. Loopback-only dev servers' },
    {
      kind: 'p',
      text: 'Both `serve` entry points — the [HTTP server](/docs/server) (`adk_rs::server::serve_with`) and the [A2A bridge](/docs/a2a) (`adk_rs::a2a::serve_with`) — refuse to bind a non-loopback address when no auth token is configured. The error is immediate and explicit; nothing listens.',
    },
    {
      kind: 'list',
      items: [
        '**Opt-out 1 (preferred)**: configure a bearer token — `AppState::with_bearer_token(...)` for the HTTP server, `A2aServerConfig::with_bearer_token(...)` for A2A, or `--auth-token` / `ADK_WEB_TOKEN` on the [CLI](/docs/cli). Every request must then carry `Authorization: Bearer <token>`.',
        '**Opt-out 2 (deliberate)**: pass `ServeOptions { dangerously_allow_unauthenticated_remote: true }` (CLI: `--dangerously-allow-unauthenticated-remote`). The name is the warning.',
        '**Never silent**: even with auth configured, binding a non-loopback interface logs a `warn` line describing exactly who can now reach the agents.',
      ],
    },
    {
      kind: 'p',
      text: 'Token comparison is constant-time: the middleware XOR-folds the byte difference across the full token rather than short-circuiting, so timing cannot leak a prefix match. (Length mismatch returns early — token length is fixed per deployment, not a secret.) Failed auth returns `401` with a `WWW-Authenticate: Bearer` header.',
    },
    { kind: 'h2', text: '3. Filesystem artifact path sanitization' },
    {
      kind: 'p',
      text: '`FileArtifactService` ([artifacts](/docs/artifacts)) builds paths as `<root>/<app>/<user>/<session>/<filename>/vNNNNNN.json` — four components that can all be attacker-influenced through the HTTP API. Each component is passed through `sanitize`, which:',
    },
    {
      kind: 'list',
      items: [
        'replaces every character that is not alphanumeric, `_`, `-`, or `.` with `_`,',
        'rewrites dot-only components (`.`, `..`, `...`, …) to `_`, so `..` segments can never climb out of the artifact root, and',
        'collapses empty input to `_`, because `Path::join("")` is a no-op that would silently merge two adjacent components.',
      ],
    },
    {
      kind: 'p',
      text: 'There is no opt-out: sanitization is unconditional on the filesystem backend. If you need raw names, store them in artifact metadata, not in the path.',
    },
    { kind: 'h2', text: '4. Locked-down container code execution' },
    {
      kind: 'p',
      text: '`ContainerCodeExecutor` (feature `code-exec-docker`, see [Code execution](/docs/code-execution)) runs each call in a fresh ephemeral container with a hardened `docker run` argv by default:',
    },
    {
      kind: 'table',
      head: ['Flag', 'Default', 'Relax with'],
      rows: [
        ['`--network=none`', 'no outbound network', '`with_extra_args` (deliberate)'],
        ['`--read-only` + `--tmpfs=/tmp:rw,exec,size=64m`', 'read-only rootfs, small writable /tmp', '`with_extra_args`'],
        ['`--memory` / `--memory-swap`', '`256m` (swap pinned to the same value)', '`with_memory("1g")`'],
        ['`--cpus`', '`1.0`', '`with_cpus("0.5")`'],
        ['`--pids-limit`', '`128` (fork-bomb cap)', '`with_pids_limit(n)`'],
        ['`--user`', '`65534:65534` (nobody, never root)', '`with_user("uid:gid")`'],
        ['`--cap-drop=ALL` + `--security-opt=no-new-privileges`', 'on', 'the `drop_capabilities` field'],
      ],
    },
    {
      kind: 'p',
      text: 'Every cap is a typed `with_*` builder method, so loosening the sandbox is an explicit, reviewable line of code. The argv builder (`build_run_args`) is public so tests can assert the policy. By contrast, `LocalCodeExecutor` is subprocess isolation only — the crate documents it as **not a security boundary**.',
    },
    { kind: 'h2', text: 'What trips, at a glance' },
    {
      kind: 'table',
      head: ['You do this', 'What happens', 'Deliberate opt-out'],
      rows: [
        ['Point a provider / RestApiTool / MCP / A2A client with credentials at `http://api.example.com`', '`Error::Config` at construction', 'Use HTTPS or a loopback proxy'],
        ['`serve` on `0.0.0.0` with no token', '`Error::Config`, nothing binds', 'Bearer token, or `dangerously_allow_unauthenticated_remote`'],
        ['Register an A2A push webhook at a public `http://` URL', 'JSON-RPC `-32602 INVALID_PARAMS`', 'Use an HTTPS webhook receiver'],
        ['Send `filename: "../../etc/cron.d/x"` to the artifact API', 'Component rewritten to `_`; write stays under the root', 'None'],
        ['Run model-emitted code in Docker', 'No network, read-only rootfs, nobody user, resource caps', 'Typed `with_*` builders'],
      ],
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[HTTP server](/docs/server) — `ServeOptions` and the bearer middleware in context.',
        '[A2A protocol](/docs/a2a) — the second server with the same bind policy.',
        '[Code execution](/docs/code-execution) — executor APIs and retry behaviour.',
        '[Auth](/docs/auth) — credential storage and OAuth flows (a separate concern from transport security).',
      ],
    },
  ],
};
