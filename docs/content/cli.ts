import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'cli',
  title: 'Embedded CLI',
  description:
    'Build your own agent binary on adk_rs::cli::App and get run, web, eval, and version subcommands out of the box.',
  srcPath: 'src/cli.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Rust binaries are statically linked — agents cannot be discovered and loaded at runtime — so adk-rs does not ship a single prebuilt `adk` binary. Instead, the `cli` feature provides library scaffolding: you write a tiny `main.rs` that registers agents on `adk_rs::cli::App` and forwards to `App::run`, producing a statically linked binary with a full clap-based CLI.',
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'The `cli` feature pulls in `clap` plus the `telemetry`, `server`, and `eval` features, so one flag gives you logging setup, the dev HTTP server, and eval-set replay.',
    },
    { kind: 'h2', text: 'The App type' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn new(name: impl Into<String>) -> App',
          desc: 'Construct an empty app. `name` becomes the `app_name` of every Runner the CLI builds.',
        },
        {
          sig: 'fn register(self, name: impl Into<String>, agent: Arc<dyn BaseAgent>) -> Self',
          desc: 'Register an agent under a name. `run --agent <name>` and the `web` server address agents by this name.',
        },
        {
          sig: 'fn run(self) -> Result<()>',
          desc: 'Parse `std::env::args`, initialise [telemetry](/docs/telemetry) from the global flags, build a multi-threaded Tokio runtime, and dispatch the subcommand.',
        },
        {
          sig: 'async fn run_async(self, cmd: Command) -> Result<()>',
          desc: 'Async dispatch with an explicit `Command` value — useful for tests that drive the CLI without a process boundary.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'For each invocation the CLI builds a `Runner` with an `InMemorySessionService` and `auto_create_session(true)`. Sessions therefore do not persist across process runs; embed the [server](/docs/server) directly with a [persistent session service](/docs/sessions-and-state) when you need durability.',
    },
    { kind: 'h2', text: 'Example main.rs' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/main.rs',
      code: `use std::sync::Arc;

fn main() -> adk_rs::Result<()> {
    adk_rs::cli::App::new("my-app")
        .register("greeter", Arc::new(build_greeter()?))
        .run()
}`,
    },
    { kind: 'h2', text: 'Global flags' },
    {
      kind: 'table',
      head: ['Flag', 'Default', 'Purpose'],
      rows: [
        ['`--log <FILTER>`', '`info` (env: `ADK_LOG`)', '`RUST_LOG`-style tracing filter, e.g. `adk_rs=debug,info`.'],
        ['`--log-format <FORMAT>`', '`compact`', 'Log output format: `compact`, `pretty`, or `json`.'],
      ],
    },
    { kind: 'h2', text: 'Subcommands' },
    { kind: 'h3', text: 'run — one user turn' },
    {
      kind: 'table',
      head: ['Flag / arg', 'Default', 'Purpose'],
      rows: [
        ['`--agent <NAME>`', 'required', 'Which registered agent to run.'],
        ['`--user <ID>`', '`anonymous`', 'User id for the session.'],
        ['`--session <ID>`', 'none', 'Optional session id (auto-created if missing).'],
        ['`<MESSAGE>`', 'required', 'Positional: the user message.'],
      ],
    },
    {
      kind: 'p',
      text: '`run` streams the agent’s events and prints the concatenated text of every content-bearing event to stdout.',
    },
    { kind: 'h3', text: 'web — the dev HTTP server' },
    {
      kind: 'table',
      head: ['Flag', 'Default', 'Purpose'],
      rows: [
        ['`--bind <ADDR>`', '`127.0.0.1:8000`', 'Listen address.'],
        ['`--auth-token <TOKEN>`', 'none (env: `ADK_WEB_TOKEN`)', 'Require `Authorization: Bearer <token>` on every request. Recommended whenever `--bind` is not loopback.'],
        ['`--dangerously-allow-unauthenticated-remote`', 'off', 'Bind a non-loopback address without a token. Refused by default — see [Security model](/docs/security).'],
        ['`--allow-origins <ORIGIN>`', 'none (repeatable)', 'CORS allow-list, e.g. `http://localhost:4200` for the adk-web UI.'],
      ],
    },
    {
      kind: 'p',
      text: '`web` registers a Runner per agent and delegates to `adk_rs::server::serve_with` — the full [`adk api_server`-compatible endpoint surface](/docs/server) including `/run`, `/run_sse`, and session CRUD.',
    },
    { kind: 'h3', text: 'eval — replay an eval set' },
    {
      kind: 'table',
      head: ['Flag', 'Default', 'Purpose'],
      rows: [
        ['`--set <PATH>`', 'required', 'Path to the JSON eval set (see [Evaluation](/docs/eval) for the format).'],
        ['`--agent <NAME>`', 'required', 'Which registered agent to evaluate.'],
      ],
    },
    {
      kind: 'p',
      text: '`eval` loads the set, runs it through an `EvalRunner` with `TrajectoryMatch::new(1.0)` and `ResponseMatch::new(0.5)` as the metric pair and `eval-user` as the user id, then prints the `EvalReport` as pretty JSON. See [Evaluation](/docs/eval) for the format and metrics.',
    },
    { kind: 'h3', text: 'version' },
    {
      kind: 'p',
      text: 'Prints `adk-rs <crate version>`. The clap derive also provides the standard `--version` and `--help` flags at every level.',
    },
    { kind: 'h2', text: 'Shell session' },
    {
      kind: 'code',
      lang: 'bash',
      title: 'Using the generated binary',
      code: `my-app run --agent greeter "Hello!"        # single-turn invocation
my-app web --bind 127.0.0.1:8000           # dev server, loopback default
my-app web --bind 0.0.0.0:8000 \\
  --auth-token "$ADK_WEB_TOKEN"            # non-loopback bind requires auth
my-app web --allow-origins http://localhost:4200   # for the adk-web UI
my-app eval --agent greeter --set hello.evalset.json
my-app version`,
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[HTTP server](/docs/server) — what `web` actually serves.',
        '[Evaluation](/docs/eval) — eval-set format and metrics.',
        '[Telemetry](/docs/telemetry) — what `--log` and `--log-format` configure.',
      ],
    },
  ],
};
