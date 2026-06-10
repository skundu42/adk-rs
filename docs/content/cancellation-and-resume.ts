import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'cancellation-and-resume',
  title: 'Cancellation & resume',
  description:
    'Cooperatively cancel in-flight invocations, and resume paused ones in place with checkpointed workflow agents.',
  srcPath: 'src/core/cancel.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Every invocation carries a cheap, cloneable `CancellationToken`; flipping it makes agents exit cleanly at the next safe point. Pair that with `ResumabilityConfig` and a paused invocation — waiting on a tool confirmation, an auth consent, or a long-running tool — can be continued *in place* with `Runner::resume`, keeping its `invocation_id` and skipping already-completed pipeline steps.',
    },
    { kind: 'h2', text: 'CancellationToken' },
    {
      kind: 'p',
      text: 'The token wraps a single `Arc<AtomicBool>` — all clones share the same state, `cancel()` is idempotent, and checking costs one atomic load. The crate deliberately avoids `tokio_util`\'s token: agents observe the flag synchronously between awaits, so an `AtomicBool` suffices.',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'CancellationToken::new() -> Self', desc: 'Fresh token in the "not cancelled" state.' },
        { sig: 'CancellationToken::cancel(&self)', desc: 'Flip to "cancelled". Idempotent; visible to every clone.' },
        { sig: 'CancellationToken::is_cancelled(&self) -> bool', desc: 'True once any clone has been cancelled.' },
        { sig: 'InvocationContext::is_cancelled(&self) -> bool', desc: 'Convenience check agents call at safe points — between iterations of the LLM↔tool loop and between sub-agents.' },
      ],
    },
    {
      kind: 'p',
      text: 'Cancellation is **cooperative**: an `LlmAgent` checks the flag at the top of each loop iteration, before issuing the next LLM call. A tool already in flight runs to completion, but no further turns are issued. The agent then emits a terminal event with `error_code: "CANCELLED"` and `error_message: "invocation was cancelled"`, so consumers can distinguish a cancel from an organic stop.',
    },
    { kind: 'h2', text: 'Starting, observing, cancelling' },
    {
      kind: 'p',
      text: '`Runner::run`/`run_with` return just the event stream. When you need the id — to cancel from another task or HTTP handler — use `Runner::start`, which returns a `RunningInvocation` handle. The runner keeps an internal map of in-flight invocations; entries are registered in `start` and removed automatically when the stream ends, however it ends.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'Runner::start(&self, user_id, session_id: Option<&str>, user_content: Content, run_config: RunConfig) -> Result<RunningInvocation>',
          desc: 'Start an invocation and return its handle.',
        },
        {
          sig: 'struct RunningInvocation { invocation_id: String, cancellation: CancellationToken, events: EventStream<\'static> }',
          desc: 'Handle for an in-flight invocation: the server-assigned id, a clone of the shared cancellation token, and the agent\'s event stream.',
        },
        {
          sig: 'Runner::cancel(&self, invocation_id: &str) -> bool',
          desc: 'Flip the matching invocation\'s token. Returns `false` if the id is unknown (finished or never started).',
        },
        {
          sig: 'Runner::is_active(&self, invocation_id: &str) -> bool',
          desc: 'Whether an invocation with this id is currently registered as in-flight.',
        },
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Cancel from outside the stream',
      code: `use adk_rs::core::RunConfig;
use adk_rs::genai_types::Content;
use futures::StreamExt;

let handle = runner
    .start("user", None, Content::user_text("research this topic"), RunConfig::default())
    .await?;
let invocation_id = handle.invocation_id.clone();

// Elsewhere (another task, an HTTP DELETE handler, ...):
assert!(runner.cancel(&invocation_id));
// Equivalent, with the handle in scope: handle.cancellation.cancel();

let mut events = handle.events;
while let Some(event) = events.next().await {
    let event = event?;
    if event.response.error_code.as_deref() == Some("CANCELLED") {
        println!("stopped before the next LLM call");
    }
}`,
    },
    { kind: 'h2', text: 'What pauses an invocation' },
    {
      kind: 'p',
      text: 'Three gates make an agent stop issuing turns and hand control back to the caller, marking the invocation paused. In each case the pausing event carries the call id in `long_running_tool_ids`, and the caller resumes by resubmitting a `FunctionResponse` with that id:',
    },
    {
      kind: 'list',
      items: [
        '**[Tool confirmation](/docs/tool-confirmation)** — a `require_confirmation` tool emits `adk_request_confirmation` and waits for the human\'s decision.',
        '**[Auth consent](/docs/auth)** — a tool whose credential needs interactive OAuth2 consent emits `adk_request_credential`.',
        '**Long-running tools** — a tool marked `is_long_running` returns a handle; the caller later resubmits the final result.',
      ],
    },
    { kind: 'h2', text: 'Resumability and checkpoints' },
    {
      kind: 'p',
      text: 'By default a follow-up turn is a *new* invocation: a `SequentialAgent` pipeline re-runs from its first sub-agent (replay of the pending call is author-scoped, so earlier agents skip it safely). With resumability enabled, workflow agents instead record checkpoints as sub-agents complete — events whose `actions.agent_state` holds `{"completed_sub_agents": n}` — and `Runner::resume` continues the *same* `invocation_id` from the last checkpoint, never re-running finished steps.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'RunnerBuilder::resumable(self, yes: bool) -> Self',
          desc: 'App-level switch; sets `ResumabilityConfig { is_resumable }`. Copied into each invocation\'s `RunConfig` unless the caller set `run_config.resumability` explicitly.',
        },
        {
          sig: 'Runner::resume(&self, user_id: &str, session_id: &str, invocation_id: &str, new_content: Option<Content>, run_config: RunConfig) -> Result<RunningInvocation>',
          desc: 'Resume a paused invocation in place. `new_content` typically carries the unblocking `FunctionResponse`: a long-running result, an `adk_request_confirmation` decision, or an `adk_request_credential` consent.',
        },
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Pause on a confirmation gate, resume in place',
      code: `let runner = Runner::builder()
    .app_name("ops")
    .agent(pipeline) // SequentialAgent: [plan, deploy-with-confirmation]
    .session_service(svc.clone())
    .resumable(true)
    .build()?;

let handle = runner
    .start("user", None, Content::user_text("ship it"), RunConfig::default())
    .await?;
let invocation_id = handle.invocation_id.clone();
handle.events.collect::<Vec<_>>().await; // pauses on the gate

// Later: resume the SAME invocation with the approval. Step one's
// checkpoint means it is not re-run.
let resumed = runner
    .resume("user", &session_id, &invocation_id,
            Some(approval_content), RunConfig::default())
    .await?;
assert_eq!(resumed.invocation_id, invocation_id);`,
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'Plain conversation continuation needs none of this machinery. Sessions are append-only event logs, so running with an existing `session_id` (`runner.run("user", Some(&sid), "and then?")`) starts a fresh invocation that sees the full history. `resume` is specifically for continuing a *paused* invocation under its original id.',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Tool confirmation](/docs/tool-confirmation) — the HITL gate in detail.',
        '[Authenticated tools](/docs/auth) — the consent pause and `adk_request_credential`.',
        '[Runner](/docs/runner) — the full orchestration surface.',
        '[Sessions & state](/docs/sessions-and-state) — append-only sessions and persistence backends.',
      ],
    },
  ],
};
