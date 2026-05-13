//! Code-execution demo: a model that emits `print(...)` and a local
//! subprocess runs it.
//!
//! Run:
//!     cargo run --example code_agent --features "code-exec,testing"

use std::sync::Arc;

use adk_rs::agents::LlmAgent;
use adk_rs::code_exec::local::LocalCodeExecutor;
use adk_rs::core::testing::MockModel;
use adk_rs::core::{LlmResponse, Model, SessionService};
use adk_rs::genai_types::part::ExecutableCode;
use adk_rs::genai_types::{Content, Part, Role};
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    // Pre-script a model that emits a single ExecutableCode part.
    let model = Arc::new(MockModel::new("mock-code"));
    model.push_response(LlmResponse {
        content: Some(Content {
            role: Role::Model,
            parts: vec![Part::ExecutableCode(ExecutableCode {
                language: "shell".into(),
                code: "echo hello && echo 'computed in container'".into(),
            })],
        }),
        ..LlmResponse::default()
    });
    // Second turn: model summarises the result.
    model.push_text("I ran the script and got 'hello' plus a summary line.");

    let executor = Arc::new(
        LocalCodeExecutor::new()
            .with_interpreter("/bin/sh")
            .with_args(vec!["-s".into()]),
    );

    let agent = Arc::new(
        LlmAgent::builder("coder")
            .model(model as Arc<dyn Model>)
            .instruction("Solve problems by emitting shell snippets and explaining the output.")
            .code_executor(executor)
            .build()?,
    );
    let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    let runner = Runner::builder()
        .app_name("code-demo")
        .agent(agent)
        .session_service(svc)
        .build()?;

    let mut stream = runner.run("user", None, "Print a greeting.").await?;
    while let Some(ev) = stream.next().await {
        let ev = ev?;
        if let Some(c) = &ev.response.content {
            for p in &c.parts {
                match p {
                    Part::Text(t) => println!("[{}] {t}", ev.author),
                    Part::ExecutableCode(ec) => {
                        println!("[{}] code ({})\n{}", ev.author, ec.language, ec.code);
                    }
                    Part::CodeExecutionResult(r) => {
                        println!(
                            "[{}] result outcome={:?} output={:?}",
                            ev.author, r.outcome, r.output
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
