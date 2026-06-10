import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'examples/gemini-chat',
  title: 'Example: Gemini chat',
  description:
    'The minimal adk-rs program: one LlmAgent backed by Gemini, one runner, one streamed turn printed to stdout.',
  srcPath: 'examples/gemini_chat.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'gemini_chat is the smallest runnable agent in the repository — a single LlmAgent talking to Gemini for one turn. If you understand its forty lines, you understand the skeleton every adk-rs program shares.',
    },
    { kind: 'h2', text: 'What it demonstrates' },
    {
      kind: 'list',
      items: [
        'Constructing a provider client from the environment with `Gemini::from_env`.',
        'Building an [`LlmAgent`](/docs/llm-agent) with a name, description, model, and instruction.',
        'Assembling a [`Runner`](/docs/runner) with an in-memory `SessionService`.',
        'Consuming the [event stream](/docs/events) with `futures::StreamExt` and extracting text via `Content::text_concat`.',
      ],
    },
    { kind: 'h2', text: 'Run it' },
    {
      kind: 'code',
      lang: 'bash',
      code: `export GOOGLE_API_KEY="your-key"
cargo run --example gemini_chat --features gemini`,
    },
    {
      kind: 'p',
      text: 'The example requires the `gemini` feature (declared in the `[[example]]` section of `Cargo.toml`) and the `GOOGLE_API_KEY` environment variable; `Gemini::from_env` returns a configuration error if it is unset.',
    },
    { kind: 'h2', text: 'Full source' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'examples/gemini_chat.rs',
      code: `use std::sync::Arc;

use adk_rs::agents::LlmAgent;
use adk_rs::core::{Model, SessionService};
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;

#[tokio::main]
async fn main() -> adk_rs::error::Result<()> {
    let model: Arc<dyn Model> = Arc::new(Gemini::from_env("gemini-2.5-flash")?);
    let agent = Arc::new(
        LlmAgent::builder("greeter")
            .description("A friendly greeter")
            .model(model)
            .instruction("You greet the user warmly and concisely.")
            .build()?,
    );
    let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    let runner = Runner::builder()
        .app_name("hello")
        .agent(agent)
        .session_service(svc)
        .build()?;
    let mut stream = runner.run("u", None, "Hello!").await?;
    while let Some(ev) = stream.next().await {
        let ev = ev?;
        if let Some(c) = ev.response.content {
            let text = c.text_concat();
            if !text.is_empty() {
                println!("[{}] {}", ev.author, text);
            }
        }
    }
    Ok(())
}`,
    },
    { kind: 'h2', text: 'Setup: the model' },
    {
      kind: 'p',
      text: '`Gemini::from_env("gemini-2.5-flash")` reads `GOOGLE_API_KEY` and yields a client speaking the Gemini REST API with SSE streaming. The agent never sees the concrete type — it is erased to `Arc<dyn Model>`, the single trait every [provider](/docs/providers) implements, which is what makes [swapping providers](/docs/examples/three-providers) a one-line change.',
    },
    { kind: 'h2', text: 'Agent and runner construction' },
    {
      kind: 'p',
      text: 'The `LlmAgent::builder("greeter")` chain sets a `description` (used by parent agents for [delegation](/docs/multi-agent); informational here) and an `instruction` that becomes the system prompt. `build()` validates and returns the agent as a value.',
    },
    {
      kind: 'p',
      text: 'The `Runner` is the orchestrator: it owns the [session service](/docs/sessions-and-state), appends every event to the session, and drives the agent. `InMemorySessionService` is the zero-setup backend — state lives only as long as the process.',
    },
    { kind: 'h2', text: 'The event loop' },
    {
      kind: 'p',
      text: '`runner.run("u", None, "Hello!")` runs one turn for user `"u"`; the `None` session id auto-creates a fresh session. The returned `EventStream` is a `Stream` of `Result<Event>`. The loop unwraps each event, takes `ev.response.content` (an `Option<Content>`), and prints non-empty text tagged with `ev.author` — the name of the agent that produced the event.',
    },
    {
      kind: 'code',
      lang: 'text',
      title: 'expected output',
      code: `[greeter] Hello! It's lovely to hear from you.`,
    },
    {
      kind: 'p',
      text: 'A plain-text turn yields a single model event, so you see exactly one line (the wording varies by model run).',
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'The module doc comment at the top of the source still mentions `cargo run -p adk-examples --bin gemini_chat`, a leftover from the pre-0.2 workspace layout. The correct invocation is the `cargo run --example gemini_chat --features gemini` command above.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Quickstart](/docs/quickstart) — this program built from scratch, then extended with a tool.',
        '[LLM agent](/docs/llm-agent) — every builder option.',
        '[Runner](/docs/runner) — `run`, `run_with`, `start`, and cancellation.',
        '[Example: Weather agent](/docs/examples/weather-agent) — the next step: add a `#[tool]` function.',
      ],
    },
  ],
};
