import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'guides/persistent-sessions',
  title: 'Guide: Persistent sessions',
  description:
    'Move an agent from in-memory storage to SQLite-backed sessions, scoped long-lived state, filesystem artifacts, and recallable memory.',
  blocks: [
    {
      kind: 'lede',
      text: 'The in-memory services that make quickstarts pleasant make production agents amnesiac: one process restart and every conversation, fact, and file is gone. This guide swaps each volatile service for a durable one — SQLite sessions, user-scoped state, filesystem artifacts, and long-term memory — one step at a time.',
    },
    { kind: 'h2', text: '1. Why in-memory loses everything' },
    {
      kind: 'p',
      text: 'A [session](/docs/sessions-and-state) is an append-only event log plus a state map, owned by whatever `SessionService` you hand the [`Runner`](/docs/runner). `InMemorySessionService` keeps both in process memory — perfect for tests, fatal for anything users return to. The fix is purely configurational: the `SessionService` trait is the same, so your agent code does not change at all.',
    },
    { kind: 'h2', text: '2. Enable sqlite and connect' },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[dependencies]
adk-rs = { version = "0.3", features = ["gemini", "sqlite", "fs"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"
serde_json = "1"`,
    },
    {
      kind: 'p',
      text: '`SqliteSessionService::connect` takes a sqlx-style URL, opens a small connection pool, and **runs the bundled schema migration automatically** — there is no separate setup step. The schema is idempotent (`CREATE TABLE IF NOT EXISTS ...`), so reconnecting to an existing database is safe. (A `postgres` feature exports `PostgresSessionService` with the same API.)',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `use adk_rs::core::SessionService;
use adk_rs::runner::Runner;
use adk_rs::services::sql::SqliteSessionService;
use std::sync::Arc;

// "mode=rwc" = read/write/create: sqlx creates the file if it is missing.
// In tests, "sqlite::memory:" gives you a throwaway database.
let sessions: Arc<dyn SessionService> =
    Arc::new(SqliteSessionService::connect("sqlite://adk.db?mode=rwc").await?);

let runner = Runner::builder()
    .app_name("assistant")
    .agent(agent)
    .session_service(sessions.clone())
    .auto_create_session(true) // create sessions under ids *you* choose
    .build()?;`,
    },
    { kind: 'h2', text: '3. Reuse session ids across restarts' },
    {
      kind: 'p',
      text: 'Continuing a conversation needs no special API. Sessions are keyed by `(app_name, user_id, session_id)`; pass the same id on every turn and the runner loads the stored event log, so the model sees the full prior history — even if the process restarted in between. With `auto_create_session(true)`, an unknown id is created rather than rejected.',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `// The id is just a string — derive it from your own conversation model.
let session_id = "alice-main";

// Run 1 (today):
let mut events = runner.run("alice", Some(session_id), "My dog is called Bruno.").await?;
while let Some(ev) = events.next().await { ev?; }

// ... process exits, redeploys, restarts ...

// Run 2 (tomorrow, fresh process): same id, full history.
let mut events = runner.run("alice", Some(session_id), "What is my dog called?").await?;
while let Some(ev) = events.next().await {
    if let Some(c) = ev?.response.content {
        println!("{}", c.text_concat()); // "Bruno."
    }
}`,
    },
    { kind: 'h2', text: '4. Long-lived facts: the user: and app: scopes' },
    {
      kind: 'p',
      text: 'Session-scoped state dies with the session’s relevance. For facts that should outlive one conversation, prefix the key — the SQLite backend routes each scope to its own table and overlays them on read, so `user:` keys are visible from **every** session of that user:',
    },
    {
      kind: 'table',
      head: ['Prefix', 'Visible to', 'Stored in (SQLite)'],
      rows: [
        ['`app:`', 'every user and session of the app', '`app_state` table'],
        ['`user:`', 'every session of one user', '`user_state` table'],
        ['*(none)*', 'this session only', '`sessions.state` column'],
        ['`temp:`', 'the current invocation', 'never persisted'],
      ],
    },
    {
      kind: 'p',
      text: 'Seed scoped state when creating a session, or let an agent write it by giving `output_key` a prefixed name — the session service partitions each event’s `state_delta` by scope when persisting:',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `use adk_rs::core::State;
use serde_json::json;

// Seed at creation time:
let mut state = State::new();
state.set("user:name", json!("Alice"));
state.set("app:brand_voice", json!("friendly, concise"));
sessions
    .create_session("assistant", "alice", Some(state), Some("alice-main"))
    .await?;

// Or let an agent maintain the fact itself:
let profiler = LlmAgent::builder("profiler")
    .model(model.clone())
    .instruction("Summarize everything learned about this user in one short paragraph.")
    .output_key("user:profile") // lands in user_state, shared across sessions
    .build()?;`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'Read scoped keys back through instruction templating: `.instruction("You assist {user:name?}. Profile: {user:profile?}.")`. The `?` suffix makes a placeholder optional — it renders as the empty string when the key is missing instead of failing the turn.',
    },
    { kind: 'h2', text: '5. Add the filesystem artifact service' },
    {
      kind: 'p',
      text: 'Binary or large outputs belong in [artifacts](/docs/artifacts), not state. The `fs` feature provides `FileArtifactService`, which stores each artifact as versioned JSON files under `<root>/<app>/<user>/<session>/<filename>/v000001.json` (path components are sanitized, so hostile names cannot escape the root). Tools save and load artifacts through their `ToolContext`; agents can pull artifact text into an instruction with `{artifact.<filename>}` templating or list files via `adk_rs::tools::load_artifacts_tool()`.',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `use adk_rs::services::fs::FileArtifactService;

let runner = Runner::builder()
    .app_name("assistant")
    .agent(agent)
    .session_service(sessions.clone())
    .artifact_service(Arc::new(FileArtifactService::new("./artifacts")))
    .auto_create_session(true)
    .build()?;`,
    },
    { kind: 'h2', text: '6. Optional: ingest sessions into memory' },
    {
      kind: 'p',
      text: 'Sessions answer “what happened in *this* conversation”; [memory](/docs/memory) answers “what do we know across conversations”. When a conversation wraps up, fetch it and index it with `add_session_to_memory`. To recall, register `adk_rs::tools::load_memory_tool()` on the agent and set `.memory_service(memory.clone())` on the runner — the model then calls `load_memory` with a query whenever it needs facts from past conversations.',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `use adk_rs::core::{GetSessionConfig, MemoryService};
use adk_rs::services::mem::InMemoryMemoryService;

let memory = Arc::new(InMemoryMemoryService::new());

// When a conversation is finished, fold it into long-term memory:
if let Some(finished) = sessions
    .get_session("assistant", "alice", "alice-main", GetSessionConfig::default())
    .await?
{
    memory.add_session_to_memory(&finished).await?;
}`,
    },
    {
      kind: 'callout',
      tone: 'warn',
      text: '`InMemoryMemoryService` does case-insensitive substring search and is itself volatile. For production recall, implement the `MemoryService` trait (two methods) over a vector store or search index; the sessions in SQLite remain your durable source of truth to re-ingest from.',
    },
    { kind: 'h2', text: 'Where next' },
    {
      kind: 'list',
      items: [
        '[Sessions and state](/docs/sessions-and-state) — the full state-delta and scope model.',
        '[Artifacts](/docs/artifacts) — versioning, `ToolContext` accessors, and `{artifact.*}` templating.',
        '[Memory](/docs/memory) — `load_memory` vs `preload_memory` and the service trait.',
        '[Guide: Production deployment](/docs/guides/production-deploy) — serving this durable stack over HTTP.',
      ],
    },
  ],
};
