//! Phase 1 demo: chat one turn with Gemini.
//!
//! Run with `GOOGLE_API_KEY=... cargo run -p adk-examples --bin gemini_chat`.

use std::sync::Arc;

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
}
