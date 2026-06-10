import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'examples/code-agent',
  title: 'Example: Code agent',
  description:
    'An agent whose model emits executable shell snippets that a LocalCodeExecutor runs in a subprocess, using a scripted MockModel so no API key is needed.',
  srcPath: 'examples/code_agent.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'code_agent wires a CodeExecutor into an LlmAgent: the model emits an ExecutableCode part, the framework runs it in a local subprocess, and the execution result is fed back to the model for a summary. The model is a pre-scripted MockModel, so the demo runs offline with no API key.',
    },
    { kind: 'h2', text: 'What it demonstrates' },
    {
      kind: 'list',
      items: [
        'Attaching a [code executor](/docs/code-execution) with `LlmAgent::builder(...).code_executor(...)`.',
        'Configuring `LocalCodeExecutor` away from its `python3` default to `/bin/sh -s` via `with_interpreter` / `with_args`.',
        'Scripting deterministic model turns with `adk_rs::core::testing::MockModel` (`push_response`, `push_text`) — see [Testing](/docs/testing).',
        'The code-execution event shapes: `Part::ExecutableCode` in, `Part::CodeExecutionResult` out.',
        'Pattern-matching on individual `Part` variants instead of using `text_concat`.',
      ],
    },
    { kind: 'h2', text: 'Run it' },
    {
      kind: 'code',
      lang: 'bash',
      code: `cargo run --example code_agent --features "code-exec,testing"`,
    },
    {
      kind: 'p',
      text: 'No environment variables are required: the `testing` feature exposes `MockModel` outside the crate’s own tests, and `code-exec` provides `LocalCodeExecutor`. The executor here spawns `/bin/sh`, so the demo assumes a Unix-like host.',
    },
    { kind: 'h2', text: 'Full source' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'examples/code_agent.rs',
      code: `use std::sync::Arc;

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
                        println!("[{}] code ({})\\n{}", ev.author, ec.language, ec.code);
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
}`,
    },
    { kind: 'h2', text: 'Setup: a scripted model' },
    {
      kind: 'p',
      text: '`MockModel` returns pre-queued `LlmResponse`s in order, which makes the demo deterministic and key-free. The first queued response contains a single `Part::ExecutableCode` with `language: "shell"`; the second, queued via the `push_text` shorthand, is the plain-text summary the "model" gives after seeing the execution result. A real agent would use a live provider here — the rest of the program is unchanged.',
    },
    { kind: 'h2', text: 'The executor' },
    {
      kind: 'p',
      text: '`LocalCodeExecutor::new()` defaults to `python3 -` with a 30-second timeout and 2 retries; the example overrides it to `/bin/sh -s`, so the shell reads the snippet from stdin. This executor is subprocess isolation only — **not a security boundary**. For untrusted code, use `ContainerCodeExecutor` behind the `code-exec-docker` feature, which is locked down by default (no network, read-only rootfs, memory/CPU/pids caps, non-root). See [Code execution](/docs/code-execution).',
    },
    { kind: 'h2', text: 'The execution roundtrip' },
    {
      kind: 'p',
      text: 'With `.code_executor(...)` set, the agent watches model output for `ExecutableCode` parts. When one appears, the executor runs it and the framework appends a `CodeExecutionResult` part (with an `Outcome` such as `OutcomeOk` or `OutcomeFailed`, plus captured output) before invoking the model again — mirroring the function-tool roundtrip in [weather_agent](/docs/examples/weather-agent), but for code instead of declared functions.',
    },
    { kind: 'h2', text: 'Expected output' },
    {
      kind: 'code',
      lang: 'text',
      code: `[coder] code (shell)
echo hello && echo 'computed in container'
[coder] result outcome=OutcomeOk output="hello\\ncomputed in container\\n"
[coder] I ran the script and got 'hello' plus a summary line.`,
    },
    {
      kind: 'p',
      text: 'Three phases, three event groups: the emitted code, the execution result with `outcome=OutcomeOk` and the captured stdout, and the scripted summary text.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Code execution](/docs/code-execution) — `CodeExecutor`, `LocalCodeExecutor`, and the Docker executor’s hardening flags.',
        '[Testing](/docs/testing) — `MockModel` and the `testing` feature.',
        '[Events](/docs/events) — `Part` variants and how they appear on the stream.',
        '[Security](/docs/security) — why local execution is not a sandbox.',
      ],
    },
  ],
};
