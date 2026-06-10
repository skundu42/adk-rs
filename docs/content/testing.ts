import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'testing',
  title: 'Testing agents',
  description:
    'Test agents deterministically with MockModel, in-memory services, and plain tokio tests — no network, no API keys.',
  srcPath: 'src/core/testing.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Because agents, models, and services are all traits, an adk-rs agent is testable like any other Rust value: swap the model for a scripted mock, use the in-memory session service, and assert on the event stream in an ordinary `#[tokio::test]`.',
    },
    { kind: 'h2', text: 'The testing feature' },
    {
      kind: 'p',
      text: '`adk_rs::core::testing` is compiled under `cfg(test)` for the crate’s own tests and under the `testing` cargo feature for downstream crates. Enable it in dev-dependencies so your test profile gets the helpers without shipping them:',
    },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[dependencies]
adk-rs = { version = "0.6", features = ["gemini"] }

[dev-dependencies]
adk-rs = { version = "0.6", features = ["gemini", "testing"] }`,
    },
    { kind: 'h2', text: 'MockModel' },
    {
      kind: 'p',
      text: '`MockModel` is a scripted [`Model`](/docs/models) implementation: you queue responses up front, and each `generate_content` call pops the next one in FIFO order while recording the request it received. When the queue runs dry it returns an error (`MockModel ran out of queued responses`), which makes under- and over-scripted tests fail loudly.',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'fn new(name: impl Into<String>) -> MockModel', desc: 'Construct empty. `name()` returns this string; `supported_models()` is `["mock-*"]`.' },
        { sig: 'fn push_response(&self, r: LlmResponse)', desc: 'Queue a full `LlmResponse` — function calls, `ExecutableCode` parts, error codes, anything.' },
        { sig: 'fn push_text(&self, text: impl Into<String>)', desc: 'Shorthand for queueing a plain model-text response.' },
        { sig: 'fn captured_requests(&self) -> Vec<LlmRequest>', desc: 'Every `LlmRequest` the mock received, in call order — assert on system instructions, declared tools, and history here.' },
        { sig: 'async fn generate_content(&self, req: LlmRequest) -> Result<LlmResponse>', desc: 'The `Model` impl: records `req`, pops the next queued response.' },
      ],
    },
    {
      kind: 'p',
      text: 'Queueing methods take `&self` (interior mutability), so you can keep an `Arc<MockModel>` after handing a clone to the agent and keep pushing turns or reading `captured_requests` from the test body.',
    },
    { kind: 'h2', text: 'A full agent-flow test' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'tests/greeter.rs',
      code: `use adk_rs::agents::{BaseAgent, LlmAgent};
use adk_rs::core::Model;
use adk_rs::core::testing::MockModel;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::test]
async fn greeter_replies() {
    let model = Arc::new(MockModel::new("mock"));
    model.push_text("hello there");

    let agent: Arc<dyn BaseAgent> = Arc::new(
        LlmAgent::builder("greet")
            .model(model.clone() as Arc<dyn Model>)
            .instruction("be terse")
            .build()
            .unwrap(),
    );
    let runner = Runner::builder()
        .app_name("test-app")
        .agent(agent)
        .session_service(Arc::new(InMemorySessionService::new()))
        .auto_create_session(true)
        .build()
        .unwrap();

    let mut events = runner.run("alice", None, "hi").await.unwrap();
    let mut texts = Vec::new();
    while let Some(ev) = events.next().await {
        if let Some(c) = ev.unwrap().response.content {
            texts.push(c.text_concat());
        }
    }
    assert!(texts.iter().any(|t| t == "hello there"));

    // The mock saw exactly one LLM call, carrying the instruction.
    assert_eq!(model.captured_requests().len(), 1);
}`,
    },
    { kind: 'h2', text: 'Scripting multi-turn conversations' },
    {
      kind: 'p',
      text: 'Queue one response per expected LLM call. A tool-using turn is two calls: first a response containing a `FunctionCall` part, then the post-tool summary. The same pattern scripts code execution — `examples/code_agent.rs` queues an `ExecutableCode` part followed by a summarising text turn:',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'From examples/code_agent.rs',
      code: `use adk_rs::core::LlmResponse;
use adk_rs::genai_types::part::ExecutableCode;
use adk_rs::genai_types::{Content, Part, Role};

let model = Arc::new(MockModel::new("mock-code"));
// Turn 1: the model "writes" code; the executor runs it.
model.push_response(LlmResponse {
    content: Some(Content {
        role: Role::Model,
        parts: vec![Part::ExecutableCode(ExecutableCode {
            language: "shell".into(),
            code: "echo hello".into(),
        })],
    }),
    ..LlmResponse::default()
});
// Turn 2: the model summarises the execution result.
model.push_text("I ran the script and got 'hello'.");`,
    },
    { kind: 'h2', text: 'Testing tools in isolation' },
    {
      kind: 'p',
      text: 'A tool’s `run` takes args and a `ToolContext`, and `ToolContext::new(Arc<InvocationContext>)` is public — so tools are testable without an agent or model. `adk_rs::core::testing::test_invocation_context()` returns a minimal `InvocationContext` backed by `NoopSessionService` (a do-nothing `SessionService` for tests that never touch persistence); mutate the returned value to attach services or user content before wrapping it in an `Arc`.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Driving a tool directly',
      code: `use adk_rs::core::ToolContext;
use adk_rs::core::testing::test_invocation_context;
use std::sync::Arc;

#[tokio::test]
async fn weather_tool_returns_report() {
    let mut tctx = ToolContext::new(Arc::new(test_invocation_context()));
    let tool = get_weather(); // #[tool]-generated constructor
    let out = tool
        .run(serde_json::json!({"city": "Paris"}), &mut tctx)
        .await
        .unwrap();
    assert_eq!(out["city"], "Paris");
}`,
    },
    {
      kind: 'p',
      text: 'After the call you can also assert on what the tool wrote back through the context: `tctx.state_delta`, `tctx.artifact_delta`, `tctx.transfer_to_agent`, `tctx.escalate`, and `tctx.skip_summarization` are all public fields.',
    },
    {
      kind: 'p',
      text: 'The module also ships `MockEmbedder` — a deterministic hashed bag-of-words `Embedder` for testing semantic memory offline. And the repo pins the `#[tool]` macro’s misuse diagnostics with a trybuild compile-fail suite (`tests/macro_trybuild.rs` + `tests/ui/`).',
    },
    { kind: 'h2', text: 'Provider-level HTTP testing' },
    {
      kind: 'p',
      text: 'For the layer below `Model` — wire formats, retries, SSE parsing — the crate’s own test suite uses [`wiremock`](https://crates.io/crates/wiremock) (a dev-dependency, pinned `=0.6.4`) to stand up a loopback HTTP server with canned provider responses. The [HTTPS-or-loopback guard](/docs/security) deliberately exempts loopback hosts precisely so credentialed clients can point at such mocks. The integration tests in `tests/` (e.g. `tests/server_http.rs`, `tests/a2a_roundtrip.rs`) show both patterns: `tower::ServiceExt::oneshot` against `build_router` for socketless HTTP tests, and real `TcpListener`-backed servers for end-to-end round trips.',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Evaluation](/docs/eval) — scenario-level scoring on top of the same mocks.',
        '[Function tools](/docs/function-tools) — the `#[tool]` macro and `ToolContext` surface.',
        '[Examples: code agent](/docs/examples/code-agent) — the full MockModel + code-executor walkthrough.',
      ],
    },
  ],
};
