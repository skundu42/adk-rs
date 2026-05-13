//! Phase 4 demo: run the same prompt against Gemini, Claude, and OpenAI.
//!
//! Provide whichever env vars are set; the example skips the others.

use std::sync::Arc;

use adk_rs_agents::LlmAgent;
use adk_rs_core::{Model, SessionService};
use adk_rs_providers_anthropic::Anthropic;
use adk_rs_providers_gemini::Gemini;
use adk_rs_providers_openai::OpenAi;
use adk_rs_runner::Runner;
use adk_rs_services_mem::InMemorySessionService;
use futures::StreamExt;

#[tokio::main]
async fn main() -> adk_rs_error::Result<()> {
    let mut models: Vec<(&'static str, Arc<dyn Model>)> = Vec::new();
    if std::env::var("GOOGLE_API_KEY").is_ok() {
        models.push(("Gemini", Arc::new(Gemini::from_env("gemini-2.5-flash")?)));
    }
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        models.push((
            "Claude",
            Arc::new(Anthropic::from_env("claude-3-5-sonnet")?),
        ));
    }
    if std::env::var("OPENAI_API_KEY").is_ok() {
        models.push(("OpenAI", Arc::new(OpenAi::from_env("gpt-4o-mini")?)));
    }
    if models.is_empty() {
        eprintln!(
            "no provider API keys in env (set GOOGLE_API_KEY / ANTHROPIC_API_KEY / OPENAI_API_KEY)"
        );
        return Ok(());
    }
    for (label, model) in models {
        println!("\n=== {label} ===");
        let agent = Arc::new(
            LlmAgent::builder("greeter")
                .model(model)
                .instruction("Be concise.")
                .build()?,
        );
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let runner = Runner::builder()
            .app_name("hello")
            .agent(agent)
            .session_service(svc)
            .build()?;
        let mut stream = runner
            .run("u", None, "In one short sentence: what is Rust?")
            .await?;
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if let Some(c) = ev.response.content {
                let text = c.text_concat();
                if !text.is_empty() {
                    println!("{text}");
                }
            }
        }
    }
    Ok(())
}
