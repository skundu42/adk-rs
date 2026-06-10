import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'event-compaction',
  title: 'Event compaction',
  description:
    'Summarize older session events into a compact replacement so long-running conversations stop growing LLM context without bound.',
  srcPath: 'src/runner/compaction.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Sessions are append-only, so a long-lived conversation eventually drags its entire history into every LLM call. Event compaction fixes the read side: after enough invocations accumulate, the runner asks a summarizer to compress the older window into one summary event, and history assembly replaces the covered events with that summary — the model sees *summary + recent events* instead of everything.',
    },
    { kind: 'h2', text: 'EventsCompactionConfig' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'EventsCompactionConfig::new(model: Arc<dyn Model>) -> Self',
          desc: 'Construct with the default `LlmEventSummarizer` over `model`, `compaction_interval: 5` and `overlap_size: 2`.',
        },
        {
          sig: 'compaction_interval(self, n: usize) -> Self — default 5',
          desc: 'Run compaction after this many invocations have accumulated since the previous compaction. Clamped to a minimum of 1.',
        },
        {
          sig: 'overlap_size(self, n: usize) -> Self — default 2',
          desc: 'Number of already-compacted events to re-include at the start of the next window, for continuity across summaries.',
        },
        {
          sig: 'summarizer(self, s: Arc<dyn EventSummarizer>) -> Self',
          desc: 'Swap in a custom summarizer.',
        },
      ],
    },
    { kind: 'h2', text: 'The summarizer' },
    {
      kind: 'p',
      text: 'The `EventSummarizer` trait has one method: `summarize(&self, events: &[Event]) -> Result<Option<Content>>`. Returning `Ok(None)` skips compaction for that window (e.g. nothing summarizable). The default `LlmEventSummarizer` uses **the model you pass to `EventsCompactionConfig::new`** — any `Arc<dyn Model>`, independent of the agents\' models, so you can point it at a cheap, fast model. It renders the window as a transcript (text lines, `[calls f(args)]`, `[f returned ...]`), sends it with a fixed compaction system instruction, and wraps the reply as `"[Summary of earlier conversation] …"`.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'trait EventSummarizer { async fn summarize(&self, events: &[Event]) -> Result<Option<Content>> }',
          desc: 'Produces the replacement `Content` for a window of events.',
        },
        {
          sig: 'LlmEventSummarizer::new(model: Arc<dyn Model>) -> Self',
          desc: 'Default LLM-backed summarizer.',
        },
      ],
    },
    { kind: 'h2', text: 'When and what gets compacted' },
    {
      kind: 'p',
      text: 'After each invocation completes, the runner counts the distinct invocation ids in the *tail* — the events after the most recent compaction marker. Once the tail spans at least `compaction_interval` invocations, the window is formed: `overlap_size` events from just before the tail (skipping prior compaction markers) plus the whole tail. The summarizer runs, and a marker event is appended whose `actions.compaction` carries:',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'struct EventCompaction { start_timestamp: f64, end_timestamp: f64, compacted_content: Content }',
          desc: 'Timestamps (seconds) of the earliest and latest compacted events, plus the replacement content — typically the summary.',
        },
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'Compaction is best-effort and runs *after* the invocation\'s event stream completes. Failures are logged, never surfaced — it is an optimization, not part of the run contract. The original events are never deleted; only history *assembly* changes.',
    },
    { kind: 'h2', text: 'How history is rebuilt' },
    {
      kind: 'p',
      text: 'When an `LlmAgent` assembles conversation history it calls `history_with_compaction(&session.events)` instead of mapping events to contents directly. Events whose timestamps fall inside a compaction\'s `[start_timestamp, end_timestamp]` range (and that precede the marker) are replaced by the `compacted_content`, emitted once in place of the first covered event. Marker events themselves never appear in history, and when overlapping compactions cover the same event, the newest one wins.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'pub fn history_with_compaction(events: &[Event]) -> Vec<Content>',
          desc: 'Assemble LLM history from session events, honouring compaction ranges. Exported from `adk_rs::core`.',
        },
      ],
    },
    { kind: 'h2', text: 'Example' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Compaction every 8 invocations, cheap summarizer model',
      code: `use adk_rs::core::Model;
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::{EventsCompactionConfig, Runner};
use std::sync::Arc;

let summarizer_model = Arc::new(Gemini::from_env("gemini-2.5-flash")?);

let runner = Runner::builder()
    .app_name("longchat")
    .agent(agent)
    .session_service(svc.clone())
    .compaction(
        EventsCompactionConfig::new(summarizer_model as Arc<dyn Model>)
            .compaction_interval(8)
            .overlap_size(2),
    )
    .build()?;

// ... after >= 8 invocations on one session, the session log gains a
// marker event (event.actions.compaction is Some) and subsequent turns
// send "[Summary of earlier conversation] ..." instead of the old events.`,
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Events](/docs/events) — the event model and `EventActions`.',
        '[Sessions & state](/docs/sessions-and-state) — append-only sessions and persistence.',
        '[Context caching](/docs/context-caching) — the complementary lever for large stable *prefixes*.',
      ],
    },
  ],
};
