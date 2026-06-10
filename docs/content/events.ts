import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'events',
  title: 'Events',
  description:
    'The Event type, its actions payload, and how appending an event mutates the session.',
  srcPath: 'src/core/event.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Everything an adk-rs run produces is an Event: user input, model text, tool calls and their results, state changes, transfers, escalations, and compaction summaries. Sessions are append-only logs of events, and a run is a stream of them.',
    },
    { kind: 'h2', text: 'The Event struct' },
    {
      kind: 'p',
      text: 'An `Event` wraps an `LlmResponse` (the `response` field is `#[serde(flatten)]`-ed, so content, finish reason, and error fields serialize at the top level) plus identity, ordering, and control metadata.',
    },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Meaning'],
      rows: [
        ['`id`', '`String`', 'Unique event id, auto-assigned (UUID).'],
        ['`invocation_id`', '`String`', 'The invocation this event belongs to.'],
        ['`author`', '`String`', '`"user"` or the name of the agent that produced it.'],
        ['`timestamp`', '`f64`', 'Wall-clock seconds since the epoch, sub-second precision.'],
        ['`branch`', '`Option<String>`', 'Agent-tree branch, e.g. `parent.child`, for multi-agent runs.'],
        ['`response`', '`LlmResponse`', 'Content, usage, finish reason, error code/message — flattened into the event on the wire.'],
        ['`actions`', '`EventActions`', 'Side effects to apply on append; see below.'],
        ['`long_running_tool_ids`', '`Option<Vec<String>>`', 'Ids of long-running (or gated) tool calls — a non-empty list signals a paused invocation.'],
        ['`partial`', '`Option<bool>`', '`Some(true)` marks a transient streaming chunk. Partial events are never persisted.'],
        ['`turn_complete`', '`Option<bool>`', '`Some(true)` marks the event that ends a streaming turn.'],
      ],
    },
    { kind: 'h2', text: 'Constructors and helpers' },
    {
      kind: 'api',
      entries: [
        { sig: 'fn Event::new(author: impl Into<String>, response: LlmResponse) -> Event', desc: 'Fresh id, current timestamp, default actions. `invocation_id` starts empty; the runner stamps it.' },
        { sig: 'fn Event::user_text(text: impl Into<String>) -> Event', desc: 'A `"user"`-authored event wrapping `Content::user_text(text)`.' },
        { sig: 'fn Event::model_text(author, text) -> Event', desc: 'An agent-authored event wrapping `Content::model_text(text)`.' },
        { sig: 'fn Event::new_id() -> String', desc: 'Generate a UUID event id.' },
        { sig: 'fn function_calls(&self) -> Vec<FunctionCall>', desc: 'All `FunctionCall` parts in the event content (empty if none).' },
        { sig: 'fn function_responses(&self) -> Vec<FunctionResponse>', desc: 'All `FunctionResponse` parts in the event content.' },
        { sig: 'fn is_final_response(&self) -> bool', desc: 'True for events that end an agent’s response: no function calls or responses, not partial, no trailing code result — or `skip_summarization`/non-empty `long_running_tool_ids` force it.' },
        { sig: 'fn has_trailing_code_result(&self) -> bool', desc: 'True if the last content part is a `CodeExecutionResult`.' },
      ],
    },
    { kind: 'h2', text: 'EventActions' },
    {
      kind: 'p',
      text: '`EventActions` is the control channel: agents and tools attach side effects here, and the session service applies them when the event is appended. Every field is skipped during serialization when empty or `None`.',
    },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Meaning'],
      rows: [
        ['`state_delta`', '`StateDelta`', 'Key/value updates applied to [session state](/docs/sessions-and-state) on append.'],
        ['`artifact_delta`', '`IndexMap<String, u64>`', 'Filename → new version for [artifacts](/docs/artifacts) saved during this event.'],
        ['`transfer_to_agent`', '`Option<String>`', 'Transfer control to the named agent (see [Multi-agent](/docs/multi-agent)).'],
        ['`escalate`', '`Option<bool>`', 'The agent is escalating control upward (e.g. to exit a `LoopAgent`).'],
        ['`skip_summarization`', '`Option<bool>`', 'Skip LLM summarization of a function response.'],
        ['`end_of_agent`', '`Option<bool>`', 'The current agent has finished its run.'],
        ['`compaction`', '`Option<EventCompaction>`', 'Marks a summary event that replaces a range of earlier events; carries `start_timestamp`, `end_timestamp`, and `compacted_content`. See [Event compaction](/docs/event-compaction).'],
        ['`requested_tool_confirmations`', '`IndexMap<String, ToolConfirmation>`', 'Confirmations requested by this event, keyed by function-call id. Answered with `adk_request_confirmation` function responses. See [Tool confirmation](/docs/tool-confirmation).'],
        ['`agent_state`', '`Option<serde_json::Value>`', 'Free-form agent checkpoint for [resumption](/docs/cancellation-and-resume).'],
        ['`rewind_before_invocation_id`', '`Option<String>`', 'Invocation id to rewind to, for rewind events.'],
      ],
    },
    { kind: 'h2', text: 'How appending mutates the session' },
    {
      kind: 'p',
      text: 'The free function `apply_event_to_session` in `adk_rs::core::services` is the single place where an event becomes session history. It is pure and synchronous, so backends can call it under a `parking_lot::Mutex` guard. In order, it:',
    },
    {
      kind: 'list',
      ordered: true,
      items: [
        'Returns the event untouched if `partial == Some(true)` — streaming chunks never enter the log.',
        'Copies `temp:`-scoped delta keys into the live in-memory state, so the current invocation can read them.',
        'Trims `temp:` keys out of `actions.state_delta` via `State::trim_temp_keys` — ephemeral keys are never persisted.',
        'Applies the trimmed delta to `session.state`.',
        'Bumps `session.last_update_time` and pushes the event onto `session.events`.',
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'The runner persists events through `SessionService::append_event_locked`, which wraps `apply_event_to_session` in a single short critical section over the live `Arc<Mutex<Session>>` — a race-free read-modify-write even when `ParallelAgent` sub-agents append concurrently. Backends with durable storage override it to add their own atomic write.',
    },
    {
      kind: 'p',
      text: 'A related helper, `history_with_compaction(&[Event]) -> Vec<Content>`, assembles LLM conversation history from a session log: events covered by an `EventCompaction` range are replaced by the compaction’s summary content, and when overlapping compactions cover the same event the newest one wins.',
    },
    { kind: 'h2', text: 'EventStream' },
    {
      kind: 'p',
      text: 'Agents and the runner return `EventStream<\'a>`, defined in `adk_rs::core::stream` as `Pin<Box<dyn Stream<Item = Result<Event>> + Send + \'a>>`. You consume it with ordinary `futures` combinators.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Consuming a stream and matching on parts',
      code: `use adk_rs::genai_types::Part;
use futures::StreamExt;

let mut events = runner.run("alice", Some("s1"), "What's 2 + 2?").await?;
while let Some(event) = events.next().await {
    let event = event?;

    // Structured views over the content.
    for call in event.function_calls() {
        println!("-> tool call {} {}", call.name, call.args);
    }
    for resp in event.function_responses() {
        println!("<- tool result {}: {}", resp.name, resp.response);
    }

    // Or match part-by-part.
    if let Some(content) = &event.response.content {
        for part in &content.parts {
            match part {
                Part::Text(t) => print!("{t}"),
                Part::FunctionCall(_) | Part::FunctionResponse(_) => {}
                other => println!("[{other:?}]"),
            }
        }
    }

    if event.is_final_response() {
        println!("\\n[{} done]", event.author);
    }
}`,
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[The Runner](/docs/runner) — who persists events and yields the stream.',
        '[Streaming](/docs/streaming) — `partial` / `turn_complete` semantics.',
        '[Sessions & state](/docs/sessions-and-state) — where `state_delta` lands.',
        '[Event compaction](/docs/event-compaction) — summarizing long sessions.',
      ],
    },
  ],
};
