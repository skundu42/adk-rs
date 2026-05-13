//! Phase 2 demo: an agent with a `#[adk_rs::tool]`-defined function tool.
//!
//! With `GOOGLE_API_KEY` set, asks Gemini "what's the weather in Paris?".
//! Without the key, you can still `cargo build` to check the macro expands.

use std::sync::Arc;

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

/// Look up the current weather in `args.city` (canned data for the demo).
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
}

#[tokio::main]
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
}
