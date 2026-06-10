import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'sessions-and-state',
  title: 'Sessions & state',
  description:
    'The Session data model, scoped state with app, user, and temp prefixes, the SessionService trait, and the available storage backends.',
  srcPath: 'src/core/session.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'A Session is a conversation owned by an (app_name, user_id, id) triple: an append-only log of events plus a mutable state map. State keys carry scope prefixes — app:, user:, temp: — that decide where a value lives and who can see it.',
    },
    { kind: 'h2', text: 'The Session struct' },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Meaning'],
      rows: [
        ['`id`', '`SessionId` (`String`)', 'Unique within `(app_name, user_id)`.'],
        ['`app_name`', '`String`', 'Owning application.'],
        ['`user_id`', '`String`', 'Owning user.'],
        ['`state`', '`State`', 'Mutable key/value map; keys may carry a scope prefix.'],
        ['`events`', '`Vec<Event>`', 'Every [event](/docs/events) ever appended, oldest first.'],
        ['`last_update_time`', '`f64`', 'Seconds since the epoch; bumped on every append.'],
      ],
    },
    {
      kind: 'p',
      text: '`Session::new(app_name, user_id, id)` builds an empty session. List APIs return `SessionMeta` instead — the same identity fields plus `last_update_time`, without events or state — wrapped in `ListSessionsResponse { sessions: Vec<SessionMeta> }`.',
    },
    { kind: 'h2', text: 'State, StateDelta, and scope prefixes' },
    {
      kind: 'p',
      text: '`State` (in `adk_rs::core::state`) is a transparent wrapper around an `IndexMap<String, Value>`; `StateDelta` is a plain `IndexMap<String, Value>` applied as a batch. The lexical prefix of a key determines its `StateScope`:',
    },
    {
      kind: 'table',
      head: ['Prefix', 'Scope', 'Behaviour'],
      rows: [
        ['`app:`', '`StateScope::App`', 'Shared across **all users and sessions** of an app.'],
        ['`user:`', '`StateScope::User`', 'Shared across all sessions of one `(app, user)`.'],
        ['`temp:`', '`StateScope::Temp`', 'Invocation-scoped. Visible to the live session during the run, stripped by `State::trim_temp_keys` before any event is persisted — it never survives a `get_session`.'],
        ['(none)', '`StateScope::Session`', 'Pinned to this one session.'],
      ],
    },
    {
      kind: 'api',
      entries: [
        { sig: 'fn get(&self, key: &str) -> Option<&Value>', desc: 'Borrowed lookup.' },
        { sig: 'fn set(&mut self, key, value: Value) -> Option<Value>', desc: 'Insert; returns the previous value.' },
        { sig: 'fn apply(&mut self, delta: &StateDelta)', desc: 'Merge a delta in insertion order.' },
        { sig: 'fn partition_by_scope(delta: &StateDelta) -> (app, user, session, temp)', desc: 'Split a delta into four `StateDelta`s by prefix. Backends use this to route `app:`/`user:` keys to shared storage.' },
        { sig: 'fn trim_temp_keys(delta: &StateDelta) -> StateDelta', desc: 'Drop every `temp:` key — called before persisting events.' },
        { sig: 'fn StateScope::of(key: &str) -> StateScope', desc: 'Derive the scope of a key from its prefix.' },
      ],
    },
    { kind: 'h2', text: 'The SessionService trait' },
    {
      kind: 'p',
      text: 'All persistence goes through `adk_rs::core::SessionService`. `get_session` takes a `GetSessionConfig` with two optional filters: `num_recent_events` (keep only the most recent N events) and `after_timestamp` (keep events with `timestamp >= t`).',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'async fn create_session(&self, app_name, user_id, state: Option<State>, id: Option<&str>) -> Result<Session>', desc: 'Create a session; generates an id when `id` is `None`. Initial state is routed by scope.' },
        { sig: 'async fn get_session(&self, app_name, user_id, session_id, config: GetSessionConfig) -> Result<Option<Session>>', desc: 'Fetch a session, optionally filtered.' },
        { sig: 'async fn list_sessions(&self, app_name, user_id) -> Result<ListSessionsResponse>', desc: 'List `SessionMeta` for `(app, user)`.' },
        { sig: 'async fn delete_session(&self, app_name, user_id, session_id) -> Result<()>', desc: 'Delete a session.' },
        { sig: 'async fn append_event(&self, session: &mut Session, event: Event) -> Result<Event>', desc: 'Append + persist. The default impl calls `apply_event_to_session`: temp propagation, delta trim + apply, `last_update_time` bump, push.' },
        { sig: 'async fn append_event_locked(&self, session_lock: &Arc<Mutex<Session>>, event: Event) -> Result<Event>', desc: 'Race-free read-modify-write through the live `Arc<Mutex<Session>>` — the path the [runner](/docs/runner) uses. The default applies the event under one short critical section; durable backends override to add their own atomic write.' },
        { sig: 'async fn flush(&self) -> Result<()>', desc: 'Optional flush hook for buffering backends. Default: no-op.' },
      ],
    },
    { kind: 'h2', text: 'Backends' },
    {
      kind: 'table',
      head: ['Type', 'Module', 'Feature', 'Notes'],
      rows: [
        ['`InMemorySessionService`', '`adk_rs::services::mem`', 'always on', 'Volatile `DashMap` store. Keeps dedicated `app:`/`user:` stores so scoped keys are visible across sessions; `get_session` overlays app + user + session state (session keys win).'],
        ['`SqliteSessionService`', '`adk_rs::services::sql`', '`sqlite`', '`SqliteSessionService::connect("sqlite::memory:")` or `sqlite:///path.db`; runs its migrations on connect.'],
        ['`PostgresSessionService`', '`adk_rs::services::sql`', '`postgres`', 'Same API over `postgres://` URLs.'],
        ['`SqlSessionService`', '`adk_rs::services::sql`', '`sqlite` / `postgres`', 'Compatibility alias; points at SQLite when both backend features are enabled.'],
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'The `fs` feature provides a filesystem backend for [artifacts](/docs/artifacts) only — there is no filesystem session service. For durable sessions use SQLite or PostgreSQL; see the [persistent sessions guide](/docs/guides/persistent-sessions).',
    },
    { kind: 'h2', text: 'How state flows' },
    {
      kind: 'list',
      ordered: true,
      items: [
        '**Tools** accumulate writes in `ToolContext.state_delta`, the per-call delta accumulator (see [Function tools](/docs/function-tools)).',
        '**Events** carry deltas in `EventActions.state_delta`; an [`LlmAgent`](/docs/llm-agent) with `output_key` stamps the final response text (or, with `output_schema`, the parsed JSON) into the final event’s delta.',
        '**Appending** applies the delta: `apply_event_to_session` copies `temp:` keys into the live state, trims them from the persisted delta, merges the rest into `session.state`, and pushes the event. Scope-aware backends additionally route `app:`/`user:` keys to shared storage via `State::partition_by_scope`.',
      ],
    },
    { kind: 'h2', text: 'Example: app and user scopes' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Scoped state across sessions',
      code: `use adk_rs::core::{Event, LlmResponse, SessionService};
use adk_rs::services::mem::InMemorySessionService;
use parking_lot::Mutex;
use std::sync::Arc;

let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
let s1 = svc.create_session("shop", "alice", None, None).await?;
let lock = Arc::new(Mutex::new(s1.clone()));

// Write one key per scope through an event.
let mut ev = Event::new("agent", LlmResponse::default());
ev.actions.state_delta.insert("app:catalog_rev".into(), serde_json::json!(42));
ev.actions.state_delta.insert("user:currency".into(), serde_json::json!("EUR"));
ev.actions.state_delta.insert("cart".into(), serde_json::json!(["sku-1"]));
ev.actions.state_delta.insert("temp:scratch".into(), serde_json::json!(true));
svc.append_event_locked(&lock, ev).await?;

// A different user's session sees app: but not user:/session keys.
let s2 = svc.create_session("shop", "bob", None, None).await?;
let bob = svc
    .get_session("shop", "bob", &s2.id, Default::default())
    .await?
    .unwrap();
assert_eq!(bob.state.get("app:catalog_rev"), Some(&serde_json::json!(42)));
assert!(bob.state.get("user:currency").is_none());
assert!(bob.state.get("cart").is_none());

// temp: never survives persistence.
let alice = svc
    .get_session("shop", "alice", &s1.id, Default::default())
    .await?
    .unwrap();
assert!(alice.state.get("temp:scratch").is_none());`,
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Events](/docs/events) — `EventActions.state_delta` and `apply_event_to_session` in detail.',
        '[The Runner](/docs/runner) — who calls `append_event_locked`.',
        '[Persistent sessions guide](/docs/guides/persistent-sessions) — SQLite/PostgreSQL in practice.',
        '[Memory](/docs/memory) — long-term recall across sessions.',
      ],
    },
  ],
};
