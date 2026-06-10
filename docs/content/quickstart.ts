import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'quickstart',
  title: 'Quickstart',
  description:
    'Build a Gemini-backed agent from cargo new to a working function tool, and learn what flows through the event stream.',
  blocks: [
    {
      kind: 'lede',
      text: 'In ten minutes you will create a new binary crate, wire an LlmAgent to Gemini, stream its events to stdout, and then teach it a weather tool with the #[tool] macro — touching every core concept in adk-rs along the way.',
    },
    { kind: 'h2', text: 'Create the project' },
    {
      kind: 'code',
      lang: 'bash',
      code: `cargo new hello-agent
cd hello-agent`,
    },
    {
      kind: 'p',
      text: 'Add `adk-rs` with the `gemini` and `macros` features (defaults are empty — see [Installation](/docs/installation)), plus `tokio` for the runtime, `futures` for stream combinators, and `serde`/`schemars` for the tool we add later.',
    },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[package]
name = "hello-agent"
version = "0.1.0"
edition = "2024"

[dependencies]
adk-rs = { version = "0.6", features = ["gemini", "macros"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"
serde = { version = "1", features = ["derive"] }
schemars = "0.8"`,
    },
    { kind: 'h2', text: 'Write the agent' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/main.rs',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::core::{Model, SessionService};
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
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

    let mut events = runner.run("user", None, "Hello!").await?;
    while let Some(ev) = events.next().await {
        let ev = ev?;
        if let Some(c) = ev.response.content {
            println!("[{}] {}", ev.author, c.text_concat());
        }
    }
    Ok(())
}`,
    },
    {
      kind: 'p',
      text: 'Three pieces do all the work, and they recur in every adk-rs program:',
    },
    {
      kind: 'list',
      items: [
        '**The agent.** [`LlmAgent`](/docs/llm-agent) is built with a fluent builder: a name, a [`Model`](/docs/models) implementation behind `Arc<dyn Model>` (here `Gemini::from_env`, which reads `GOOGLE_API_KEY`), and a system `instruction`. `build()` validates the configuration and returns a plain value.',
        '**The runner.** The [`Runner`](/docs/runner) owns the services and orchestrates a turn. It needs an `app_name`, the root agent, and a `SessionService` — here the always-available `InMemorySessionService`, which keeps [sessions](/docs/sessions-and-state) as append-only event logs in memory.',
        '**The event stream.** `runner.run(user_id, session_id, user_text)` starts one turn. Passing `None` as `session_id` auto-creates a session. It returns an `EventStream` — an ordinary `futures::Stream` of `Result<Event>` you consume with `StreamExt::next`. Each [`Event`](/docs/events) names its `author` and may carry model `Content`; `text_concat()` joins the text parts.',
      ],
    },
    { kind: 'h2', text: 'Run it' },
    {
      kind: 'code',
      lang: 'bash',
      code: `export GOOGLE_API_KEY="your-key"
cargo run`,
    },
    {
      kind: 'code',
      lang: 'text',
      title: 'output',
      code: `[greeter] Hello there! It's wonderful to meet you.`,
    },
    {
      kind: 'p',
      text: 'A single-text turn produces one model event. The interesting structure appears once tools enter the picture.',
    },
    { kind: 'h2', text: 'Add a function tool' },
    {
      kind: 'p',
      text: 'Annotate any async function with `#[adk_rs::tool]`. The macro derives the JSON schema from the argument struct (via `schemars::JsonSchema`), generates the `FunctionDeclaration` the model sees, and emits a constructor returning an `Arc<dyn Tool>`. The doc comments become the tool and parameter descriptions.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/main.rs (additions)',
      code: `use adk_rs::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
struct GetWeatherArgs {
    /// City name in English (e.g. "Paris").
    city: String,
}

#[derive(Serialize)]
struct WeatherReport {
    city: String,
    temp_c: f32,
    description: String,
}

/// Look up the current weather in \`args.city\`.
#[tool]
async fn get_weather(
    args: GetWeatherArgs,
    _ctx: &mut adk_rs::core::ToolContext,
) -> adk_rs::Result<WeatherReport> {
    Ok(WeatherReport {
        city: args.city,
        temp_c: 22.0,
        description: "sunny".into(),
    })
}`,
    },
    {
      kind: 'p',
      text: 'Attach the tool on the builder and adjust the instruction; everything else stays the same:',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `let agent = Arc::new(
    LlmAgent::builder("weather")
        .model(model)
        .instruction("Use the get_weather tool to answer questions about cities' weather.")
        .tool(get_weather())
        .build()?,
);
// ...
let mut events = runner.run("user", None, "What's the weather in Paris?").await?;`,
    },
    { kind: 'h2', text: 'Reading the event stream' },
    {
      kind: 'p',
      text: 'A tool-using turn is no longer one event. The model first emits a **function call**; the framework dispatches your Rust function and appends a **function response**; the model then reads the result and produces the final text. `Event` exposes helpers for the tool legs:',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `while let Some(ev) = events.next().await {
    let ev = ev?;
    if let Some(c) = ev.response.content.as_ref() {
        let text = c.text_concat();
        if !text.is_empty() {
            println!("[{}] {}", ev.author, text);
        }
    }
    for fc in ev.function_calls() {
        println!("  -> tool call: {} args={}", fc.name, fc.args);
    }
    for fr in ev.function_responses() {
        println!("  <- tool response: {} = {}", fr.name, fr.response);
    }
}`,
    },
    {
      kind: 'code',
      lang: 'text',
      title: 'output',
      code: `  -> tool call: get_weather args={"city":"Paris"}
  <- tool response: get_weather = {"city":"Paris","temp_c":22.0,"description":"sunny"}
[weather] It's currently sunny and 22°C in Paris.`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'This exact program ships in the repository as `examples/weather_agent.rs` — run it with `cargo run --example weather_agent --features "gemini,macros"`. See [Example: Weather agent](/docs/examples/weather-agent).',
    },
    { kind: 'h2', text: 'Where next' },
    {
      kind: 'list',
      items: [
        '[Agents overview](/docs/agents-overview) — the `BaseAgent` trait and workflow agents.',
        '[Function tools](/docs/function-tools) — the `#[tool]` macro and the manual `Tool` trait.',
        '[Events](/docs/events) — the full anatomy of an `Event`.',
        '[Sessions and state](/docs/sessions-and-state) — persisting conversations beyond memory.',
        '[Example: Gemini chat](/docs/examples/gemini-chat) — the minimal version of this page, annotated.',
      ],
    },
  ],
};
