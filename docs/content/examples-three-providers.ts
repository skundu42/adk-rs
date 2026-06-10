import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'examples/three-providers',
  title: 'Example: Three providers',
  description:
    'Run the same prompt against Gemini, Anthropic Claude, and an OpenAI-compatible endpoint behind the single Model trait.',
  srcPath: 'examples/three_providers.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'three_providers sends one prompt — "In one short sentence: what is Rust?" — to every provider whose API key is present in the environment. It is the proof that adk-rs agents are model-agnostic: the agent code is identical for all three; only the Arc<dyn Model> changes.',
    },
    { kind: 'h2', text: 'What it demonstrates' },
    {
      kind: 'list',
      items: [
        'All three first-party [providers](/docs/providers): `Gemini`, `Anthropic`, and `OpenAi`, each constructed with `from_env`.',
        'Type erasure to `Arc<dyn Model>` — the one trait every provider implements (see [Models](/docs/models)).',
        'Detecting available credentials at runtime and skipping providers whose keys are missing.',
        'Building a fresh agent, session service, and [`Runner`](/docs/runner) per provider in a loop.',
      ],
    },
    { kind: 'h2', text: 'Run it' },
    {
      kind: 'code',
      lang: 'bash',
      code: `export GOOGLE_API_KEY="..."      # enables the Gemini leg
export ANTHROPIC_API_KEY="..."   # enables the Claude leg
export OPENAI_API_KEY="..."      # enables the OpenAI leg
# optional: export OPENAI_BASE_URL="http://localhost:11434/v1"  # e.g. Ollama

cargo run --example three_providers --features "gemini,anthropic,openai"`,
    },
    {
      kind: 'p',
      text: 'The `[[example]]` entry requires all three provider features so the binary compiles, but at runtime each leg is optional: set whichever keys you have, and the example skips the rest. With no keys at all it prints a hint and exits.',
    },
    { kind: 'h2', text: 'Full source' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'examples/three_providers.rs',
      code: `use std::sync::Arc;

use adk_rs::agents::LlmAgent;
use adk_rs::core::{Model, SessionService};
use adk_rs::providers::anthropic::Anthropic;
use adk_rs::providers::gemini::Gemini;
use adk_rs::providers::openai::OpenAi;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;

#[tokio::main]
async fn main() -> adk_rs::error::Result<()> {
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
        println!("\\n=== {label} ===");
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
}`,
    },
    { kind: 'h2', text: 'Setup: credential detection' },
    {
      kind: 'p',
      text: 'The example builds a `Vec<(&str, Arc<dyn Model>)>` and pushes one entry per provider whose key is set: `Gemini::from_env("gemini-2.5-flash")` reads `GOOGLE_API_KEY`, `Anthropic::from_env("claude-3-5-sonnet")` reads `ANTHROPIC_API_KEY`, and `OpenAi::from_env("gpt-4o-mini")` reads `OPENAI_API_KEY` plus the optional `OPENAI_BASE_URL` override — which is how the same leg can target Azure OpenAI, Ollama, or Groq.',
    },
    {
      kind: 'p',
      text: 'Because each client erases to `Arc<dyn Model>`, the rest of the program never mentions a provider type again. This is the swap point in any adk-rs application: change the constructor, keep the agent.',
    },
    { kind: 'h2', text: 'The per-provider loop' },
    {
      kind: 'p',
      text: 'For each `(label, model)` pair the loop builds an identical `LlmAgent` (instruction `"Be concise."`), a fresh `InMemorySessionService`, and a `Runner`, then streams one turn and prints the text events. Constructing a new runner per leg keeps the three conversations fully isolated — sessions are per-runner state here.',
    },
    { kind: 'h2', text: 'Expected output' },
    {
      kind: 'code',
      lang: 'text',
      title: 'with all three keys set',
      code: `=== Gemini ===
Rust is a systems programming language focused on safety, speed, and concurrency.

=== Claude ===
Rust is a memory-safe systems programming language without a garbage collector.

=== OpenAI ===
Rust is a fast, memory-safe systems programming language.`,
    },
    {
      kind: 'p',
      text: 'Exact wording varies by model and run; legs without keys are simply absent. With no keys at all the only output is the `no provider API keys in env ...` message on stderr.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Providers](/docs/providers) — base URLs, streaming, and the HTTPS-or-loopback guard.',
        '[Models](/docs/models) — the `Model` trait that makes this swap possible.',
        '[Installation](/docs/installation) — provider features and credential env vars.',
      ],
    },
  ],
};
