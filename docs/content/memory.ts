import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'memory',
  title: 'Memory',
  description:
    'Long-term memory across sessions: the MemoryService trait, the in-memory backend, and the load_memory and preload_memory tools.',
  srcPath: 'src/core/memory.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Sessions remember one conversation; memory remembers across them. A MemoryService ingests completed sessions into a long-term store and answers free-text queries with snippets — either on demand through the load_memory tool, or automatically via preload_memory at the start of every turn.',
    },
    { kind: 'h2', text: 'MemoryEntry and SearchMemoryResponse' },
    {
      kind: 'p',
      text: 'The data model in `adk_rs::core::memory` is deliberately small. A `MemoryEntry` carries the recalled snippet as a `Content` plus optional provenance mirrored from the original [event](/docs/events):',
    },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Meaning'],
      rows: [
        ['`content`', '`Content`', 'The memory content, typically a text part.'],
        ['`author`', '`Option<String>`', 'Author of the original event (`"user"` or an agent name).'],
        ['`timestamp`', '`Option<f64>`', 'Original event timestamp in seconds.'],
      ],
    },
    {
      kind: 'p',
      text: 'Searches return a `SearchMemoryResponse { memories: Vec<MemoryEntry> }` envelope.',
    },
    { kind: 'h2', text: 'The MemoryService trait' },
    {
      kind: 'p',
      text: '`adk_rs::core::MemoryService` has exactly two methods. Ingestion is explicit — nothing is written to memory automatically; you (or the [server](/docs/server) endpoint) decide when a session is worth remembering.',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'async fn add_session_to_memory(&self, session: &Session) -> Result<()>', desc: 'Index a session’s events into long-term memory.' },
        { sig: 'async fn search_memory(&self, app_name: &str, user_id: &str, query: &str) -> Result<SearchMemoryResponse>', desc: 'Search the `(app, user)` store for entries matching `query`.' },
      ],
    },
    { kind: 'h2', text: 'InMemoryMemoryService' },
    {
      kind: 'p',
      text: 'The simplest bundled backend, `adk_rs::services::mem::InMemoryMemoryService`, keeps one bucket per `(app_name, user_id)`. `add_session_to_memory` walks the session’s events and stores one `MemoryEntry` per event with non-empty text content, preserving the author and timestamp. `search_memory` is a **case-insensitive substring match** over each entry’s text — good enough for tests and quickstarts. For semantic recall, use `VectorMemoryService` below.',
    },
    { kind: 'h2', text: 'VectorMemoryService (semantic search)' },
    {
      kind: 'p',
      text: '`adk_rs::services::mem::VectorMemoryService` swaps substring matching for embedding-based retrieval. Entries are embedded once at ingest time through the `Embedder` trait; each search embeds the query and ranks entries by **cosine similarity**, returning the top `k` above an optional similarity floor. Storage is still process-local — the upgrade is retrieval quality, not durability.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'trait Embedder { async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> }',
          desc: 'Batch text → vector. Implementations ship with the provider features: `GeminiEmbedder` (gemini) and `OpenAiEmbedder` (openai); implement it yourself to bridge any other backend. Exported from `adk_rs::core`.',
        },
        {
          sig: 'VectorMemoryService::new(embedder: Arc<dyn Embedder>) -> Self',
          desc: 'Construct with defaults: top 5 results, no similarity floor.',
        },
        {
          sig: 'with_top_k(self, k: usize) -> Self',
          desc: 'Maximum results per search.',
        },
        {
          sig: 'with_min_score(self, score: f32) -> Self',
          desc: 'Minimum cosine similarity (in [-1, 1]) for an entry to be returned.',
        },
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Semantic memory with a Gemini embedder',
      code: `use adk_rs::providers::gemini::GeminiEmbedder;
use adk_rs::services::mem::VectorMemoryService;
use std::sync::Arc;

let memory = VectorMemoryService::new(
    Arc::new(GeminiEmbedder::from_env("gemini-embedding-001")?),
)
.with_top_k(5)
.with_min_score(0.3);

// Drop-in replacement for InMemoryMemoryService:
let runner = Runner::builder()
    .app_name("hotel")
    .agent(agent)
    .session_service(sessions)
    .memory_service(Arc::new(memory))
    .build()?;`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'The `testing` feature exports `adk_rs::core::testing::MockEmbedder` — a deterministic hashed bag-of-words embedder — so you can unit-test semantic memory flows without network access or API keys.',
    },
    { kind: 'h2', text: 'The load_memory tool (active recall)' },
    {
      kind: 'p',
      text: '`adk_rs::tools::load_memory_tool()` returns a tool the **model** calls when it decides it needs prior context. It declares a single required `query` string parameter, runs `search_memory` for the current `(app, user)`, and returns `{ "memories": [...] }`. It errors with a config error if the runner has no memory service.',
    },
    { kind: 'h2', text: 'The preload_memory tool (passive recall)' },
    {
      kind: 'p',
      text: '`adk_rs::tools::preload_memory_tool(max_entries)` is a *passive* tool: its `declaration()` is `None`, so it is never advertised to the model and cannot be called. Instead it implements `process_llm_request`, which runs at turn start: it queries memory with the invocation’s user content as the search text and, when there are hits, appends a `Relevant prior context:` bullet list (capped at `max_entries`) to the request’s system text. It silently does nothing when no memory service is configured, the user content is empty, or the search returns no entries.',
    },
    {
      kind: 'table',
      head: ['Tool', 'Trigger', 'Effect'],
      rows: [
        ['`load_memory`', 'Model issues a function call with a `query`.', 'Returns matching `MemoryEntry` values as the tool result.'],
        ['`preload_memory`', 'Every turn, before the LLM call.', 'Inlines up to `max_entries` matching snippets into the system prompt.'],
      ],
    },
    { kind: 'h2', text: 'Ingesting via the HTTP server' },
    {
      kind: 'p',
      text: 'With the `server` feature, `PATCH /apps/:app/users/:user/memory` with body `{ "sessionId": "..." }` loads the named session and passes it to `add_session_to_memory`. It returns `400` when no memory service is configured and `404` when the session does not exist. See [Server](/docs/server).',
    },
    { kind: 'h2', text: 'Example: wiring it together' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Memory service + load_memory + explicit ingestion',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::core::{GetSessionConfig, MemoryService, SessionService};
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::Runner;
use adk_rs::services::mem::{InMemoryMemoryService, InMemorySessionService};
use adk_rs::tools::{load_memory_tool, preload_memory_tool};
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    let memory: Arc<dyn MemoryService> = Arc::new(InMemoryMemoryService::new());

    let agent = LlmAgent::builder("concierge")
        .model(Arc::new(Gemini::from_env("gemini-2.5-flash")?))
        .instruction("Recall prior conversations with load_memory when useful.")
        .tool(load_memory_tool())
        .tool(preload_memory_tool(5))
        .build()?;

    let runner = Runner::builder()
        .app_name("hotel")
        .agent(Arc::new(agent))
        .session_service(sessions.clone())
        .memory_service(memory.clone())
        .auto_create_session(true)
        .build()?;

    // First conversation.
    let s1 = runner
        .run("alice", Some("trip-1"), "I prefer rooms on high floors.")
        .await?;
    s1.collect::<Vec<_>>().await;

    // Ingest the finished session into long-term memory.
    let session = sessions
        .get_session("hotel", "alice", "trip-1", GetSessionConfig::default())
        .await?
        .expect("session exists");
    memory.add_session_to_memory(&session).await?;

    // A later session can now recall the preference.
    let mut s2 = runner
        .run("alice", Some("trip-2"), "Book me a room like last time.")
        .await?;
    while let Some(event) = s2.next().await {
        if let Some(content) = event?.response.content {
            println!("{}", content.text_concat());
        }
    }
    Ok(())
}`,
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Sessions & state](/docs/sessions-and-state) — short-term, per-conversation storage.',
        '[Built-in tools](/docs/builtin-tools) — the rest of the bundled toolset.',
        '[The Runner](/docs/runner) — wiring `memory_service` into the builder.',
        '[Server](/docs/server) — the memory ingest endpoint.',
      ],
    },
  ],
};
