import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'examples/weather-agent',
  title: 'Example: Weather agent',
  description:
    'An LlmAgent that answers weather questions through a function tool defined with the #[tool] proc-macro.',
  srcPath: 'examples/weather_agent.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'weather_agent extends the minimal chat loop with a function tool: a plain async Rust function annotated with #[tool] that Gemini calls to fetch (canned) weather data. It is the canonical demonstration of the tool roundtrip in the event stream.',
    },
    { kind: 'h2', text: 'What it demonstrates' },
    {
      kind: 'list',
      items: [
        'Declaring tool arguments as a `Deserialize + JsonSchema` struct, with doc comments becoming parameter descriptions.',
        'The [`#[tool]`](/docs/function-tools) macro turning an async function into an `Arc<dyn Tool>` constructor.',
        'Attaching a tool to an [`LlmAgent`](/docs/llm-agent) with `.tool(get_weather())`.',
        'Observing `function_calls()` and `function_responses()` on each [`Event`](/docs/events).',
        'Degrading gracefully: without `GOOGLE_API_KEY` the binary prints a hint and exits, so the macro expansion can still be checked with a plain build.',
      ],
    },
    { kind: 'h2', text: 'Run it' },
    {
      kind: 'code',
      lang: 'bash',
      code: `export GOOGLE_API_KEY="your-key"
cargo run --example weather_agent --features "gemini,macros"`,
    },
    {
      kind: 'p',
      text: 'The `[[example]]` entry in `Cargo.toml` requires both `gemini` (the provider) and `macros` (the `#[tool]` proc-macro, re-exported as `adk_rs::tool`).',
    },
    { kind: 'h2', text: 'Full source' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'examples/weather_agent.rs — tool definition',
      code: `use std::sync::Arc;

use adk_rs::agents::LlmAgent;
use adk_rs::core::{Model, SessionService};
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use adk_rs::tool;
use futures::StreamExt;
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

/// Look up the current weather in \`args.city\` (canned data for the demo).
#[tool]
async fn get_weather(
    args: GetWeatherArgs,
    _ctx: &mut adk_rs::core::ToolContext,
) -> adk_rs::error::Result<WeatherReport> {
    Ok(WeatherReport {
        city: args.city,
        temp_c: 22.0,
        description: "sunny".into(),
    })
}`,
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'examples/weather_agent.rs — main',
      code: `#[tokio::main]
async fn main() -> adk_rs::error::Result<()> {
    let Ok(_) = std::env::var("GOOGLE_API_KEY") else {
        eprintln!("set GOOGLE_API_KEY to run this demo against Gemini");
        return Ok(());
    };
    let model: Arc<dyn Model> = Arc::new(Gemini::from_env("gemini-2.5-flash")?);
    let agent = Arc::new(
        LlmAgent::builder("weather")
            .model(model)
            .instruction("Use the get_weather tool to answer questions about cities' weather.")
            .tool(get_weather())
            .build()?,
    );
    let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    let runner = Runner::builder()
        .app_name("weather")
        .agent(agent)
        .session_service(svc)
        .build()?;
    let mut stream = runner
        .run("u", None, "What's the weather in Paris?")
        .await?;
    while let Some(ev) = stream.next().await {
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
    }
    Ok(())
}`,
    },
    { kind: 'h2', text: 'The tool definition' },
    {
      kind: 'p',
      text: 'The macro derives everything the model needs from ordinary Rust items. `GetWeatherArgs` derives `Deserialize` (so incoming JSON arguments can be parsed) and `JsonSchema` (so the macro can emit the parameter schema); the `/// City name...` doc comment surfaces as the parameter description in the `FunctionDeclaration`. The return type only needs `Serialize`. The second parameter, `&mut ToolContext`, gives real tools access to session state, artifacts, and auth — unused here.',
    },
    {
      kind: 'p',
      text: 'After expansion, `get_weather()` is a zero-argument constructor returning `Arc<dyn Tool>`, which is exactly what `LlmAgent::builder(...).tool(...)` accepts. Manual `Tool` implementations remain available when you need full control — see [Function tools](/docs/function-tools).',
    },
    { kind: 'h2', text: 'Agent construction' },
    {
      kind: 'p',
      text: 'Identical to [gemini_chat](/docs/examples/gemini-chat) except for `.tool(get_weather())` and an instruction that tells the model when to call it. The framework handles the rest of the loop: dispatching the call, serialising the result, and feeding it back to the model for the final answer.',
    },
    { kind: 'h2', text: 'The event loop and expected output' },
    {
      kind: 'p',
      text: 'A tool-using turn produces at least three events: the model’s function call, the framework’s function response, and the model’s final text. `ev.function_calls()` and `ev.function_responses()` return the `FunctionCall` / `FunctionResponse` parts of each event, so the loop prints the whole roundtrip:',
    },
    {
      kind: 'code',
      lang: 'text',
      title: 'expected output',
      code: `  -> tool call: get_weather args={"city":"Paris"}
  <- tool response: get_weather = {"city":"Paris","temp_c":22.0,"description":"sunny"}
[weather] It's sunny in Paris right now, with a temperature of 22°C.`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'Without `GOOGLE_API_KEY` the example exits early with `set GOOGLE_API_KEY to run this demo against Gemini` — useful for verifying that the `#[tool]` macro expands and the example compiles in CI without credentials.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Function tools](/docs/function-tools) — the `#[tool]` macro in depth and the manual escape hatch.',
        '[Tools overview](/docs/tools-overview) — the `Tool` trait, toolsets, and built-ins.',
        '[Events](/docs/events) — every field on `Event`, including `function_calls` / `function_responses`.',
        '[Quickstart](/docs/quickstart) — builds this same program step by step.',
      ],
    },
  ],
};
