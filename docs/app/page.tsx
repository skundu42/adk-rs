import Link from 'next/link';
import { CodeBlock } from '@/components/CodeBlock';

const QUICKSTART = `use adk_rs::{agents::LlmAgent, providers::gemini::Gemini, runner::Runner};
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let agent = LlmAgent::builder("greeter")
        .model(Arc::new(Gemini::from_env("gemini-2.5-flash")?))
        .instruction("You greet the user warmly.")
        .build()?;
    let runner = Runner::builder()
        .app_name("hello")
        .agent(Arc::new(agent))
        .session_service(Arc::new(InMemorySessionService::new()))
        .build()?;

    let mut events = runner.run("user", None, "Hello!").await?;
    while let Some(event) = events.next().await {
        if let Some(content) = event?.response.content {
            println!("{}", content.text_concat());
        }
    }
    Ok(())
}`;

const FEATURES: { title: string; body: string; href: string }[] = [
  {
    title: 'Composable agents',
    body: 'LlmAgent, SequentialAgent, ParallelAgent and LoopAgent nest through one BaseAgent trait, driven by a unified tokio event stream.',
    href: '/docs/agents-overview',
  },
  {
    title: 'Three first-class providers',
    body: 'Gemini (REST + SSE), Anthropic Claude, and an OpenAI-compatible client that also reaches Azure, Ollama and Groq — all behind one Model trait.',
    href: '/docs/providers',
  },
  {
    title: 'The #[tool] macro',
    body: 'Annotate any async fn and the macro derives the JSON schema, FunctionDeclaration and Tool impl. Manual impls stay available.',
    href: '/docs/function-tools',
  },
  {
    title: 'Pluggable services',
    body: 'Session, memory, artifact and credential traits with in-memory, filesystem, SQLite and PostgreSQL backends out of the box.',
    href: '/docs/sessions-and-state',
  },
  {
    title: 'MCP & OpenAPI toolsets',
    body: 'Mount tools from any Model Context Protocol server (stdio or streamable HTTP) or generate one tool per operation from an OpenAPI 3.x spec.',
    href: '/docs/mcp',
  },
  {
    title: 'A2A protocol',
    body: 'Spec-compliant Agent-to-Agent JSON-RPC client and server bridge — interoperate with any A2A agent in either direction.',
    href: '/docs/a2a',
  },
  {
    title: 'Human-in-the-loop',
    body: 'Tool confirmation, interactive auth consent, long-running tools, cancellation and resumable invocations are built into the runtime.',
    href: '/docs/tool-confirmation',
  },
  {
    title: 'Sandboxed code execution',
    body: 'Run model-emitted code in a local subprocess or a locked-down Docker container: no network, read-only rootfs, dropped capabilities.',
    href: '/docs/code-execution',
  },
  {
    title: 'Serve, evaluate, observe',
    body: 'An axum dev server compatible with the adk-web UI, a JSON eval-set framework, and tracing with optional OTLP export.',
    href: '/docs/server',
  },
];

const EXAMPLES: { name: string; desc: string; href: string }[] = [
  {
    name: 'gemini_chat',
    desc: 'Minimal single-agent loop: one turn against Gemini, events streamed to stdout.',
    href: '/docs/examples/gemini-chat',
  },
  {
    name: 'weather_agent',
    desc: 'A #[tool]-defined function tool, with tool calls and responses printed live.',
    href: '/docs/examples/weather-agent',
  },
  {
    name: 'three_providers',
    desc: 'The same prompt against Gemini, Claude and OpenAI through one Model trait.',
    href: '/docs/examples/three-providers',
  },
  {
    name: 'code_agent',
    desc: 'A scripted MockModel emits shell snippets; LocalCodeExecutor runs them.',
    href: '/docs/examples/code-agent',
  },
];

export default function Home() {
  return (
    <main className="landing">
      <section className="hero">
        <div>
          <span className="hero-eyebrow">
            <span className="pulse" />
            cargo add adk-rs
          </span>
          <h1>
            Build <span className="molten">serious agents</span>
            <br />
            in Rust.
          </h1>
          <p className="hero-sub">
            adk-rs is a code-first Agent Development Kit for Rust: model-agnostic,
            deployment-agnostic agent construction with the low overhead, predictable latency and
            safety guarantees of the Rust toolchain.
          </p>
          <div className="hero-ctas">
            <Link className="btn btn-fill" href="/docs/quickstart">
              Get started →
            </Link>
            <Link className="btn btn-ghost" href="/docs/introduction">
              Read the docs
            </Link>
          </div>
          <div className="hero-stats">
            <span>
              <b>0</b> unsafe blocks
            </span>
            <span>
              <b>20</b> cargo features
            </span>
            <span>
              <b>1.85+</b> MSRV
            </span>
            <span>
              <b>Apache-2.0</b> licensed
            </span>
          </div>
        </div>
        <div className="hero-code">
          <CodeBlock code={QUICKSTART} lang="rust" title="src/main.rs" />
        </div>
      </section>

      <section className="land-section">
        <span className="land-label">Capabilities</span>
        <h2>
          Everything an agent runtime needs, <em>behind cargo features.</em>
        </h2>
        <div className="feature-grid">
          {FEATURES.map((f, i) => (
            <Link key={f.title} href={f.href} className="feature-cell" style={{ color: 'inherit' }}>
              <span className="feature-num">{String(i + 1).padStart(2, '0')} /</span>
              <h3>{f.title}</h3>
              <p>{f.body}</p>
            </Link>
          ))}
        </div>
      </section>

      <section className="land-section">
        <span className="land-label">Examples</span>
        <h2>
          Runnable demos, <em>straight from the repo.</em>
        </h2>
        <div className="example-strip">
          {EXAMPLES.map((e) => (
            <Link key={e.name} href={e.href} className="example-card">
              <span className="ex-name">examples/{e.name}.rs</span>
              <p>{e.desc}</p>
            </Link>
          ))}
        </div>
      </section>
    </main>
  );
}
