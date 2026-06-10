import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'guides/multi-agent-pipeline',
  title: 'Guide: A multi-agent pipeline',
  description:
    'Build a research-and-write pipeline from parallel specialist agents, a state-driven writer, and an optional review loop.',
  blocks: [
    {
      kind: 'lede',
      text: 'Workflow agents let you compose LLM agents the way you compose functions: fan two researchers out with ParallelAgent, hand their findings to a writer through session state, polish the result in a LoopAgent, and wrap the stages in SequentialAgent. This guide builds that pipeline end to end with real, compiling code.',
    },
    { kind: 'h2', text: '1. Set up the project' },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[dependencies]
adk-rs = { version = "0.6", features = ["gemini"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"`,
    },
    { kind: 'h2', text: '2. Fan out two researchers with ParallelAgent' },
    {
      kind: 'p',
      text: 'Each researcher is an ordinary [`LlmAgent`](/docs/llm-agent) with its own angle and its own `output_key` — when an agent produces its final response, the text is stamped into the event’s `state_delta` under that key. `ParallelAgent::new(name, description, sub_agents)` takes children as `Vec<Arc<dyn BaseAgent>>`, fails fast on an empty list, and merges their event streams as results arrive, tagging each child’s events with a branch (`researchers.0`, `researchers.1`, …).',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/main.rs (agents)',
      code: `use adk_rs::agents::{LlmAgent, LoopAgent, ParallelAgent, SequentialAgent};
use adk_rs::core::Model;
use adk_rs::providers::gemini::Gemini;
use std::sync::Arc;

fn build_pipeline(model: Arc<dyn Model>) -> adk_rs::Result<Arc<SequentialAgent>> {
    let tech = Arc::new(
        LlmAgent::builder("tech_researcher")
            .model(model.clone())
            .instruction("Give 3-5 dense bullet points on the technical state of the art for the user's topic. Facts only.")
            .output_key("tech_findings")
            .build()?,
    );

    let market = Arc::new(
        LlmAgent::builder("market_researcher")
            .model(model.clone())
            .instruction("Give 3-5 dense bullet points on adoption, players, and trends for the user's topic.")
            .output_key("market_findings")
            .build()?,
    );

    let researchers = Arc::new(ParallelAgent::new(
        "researchers",
        "Runs the research specialists concurrently",
        vec![tech, market],
    )?);`,
    },
    { kind: 'h2', text: '3. A writer that reads the researchers’ state' },
    {
      kind: 'p',
      text: 'The writer never sees the researchers’ events directly. It reads `{tech_findings}` and `{market_findings}` from session state — the templater resolves each `{key}` and errors if a required key is missing (append `?` to make one optional).',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `    let writer = Arc::new(
        LlmAgent::builder("writer")
            .model(model.clone())
            .instruction(
                "Write a concise brief on the user's topic using ONLY this research.\\n\\nTechnical findings:\\n{tech_findings}\\n\\nMarket findings:\\n{market_findings}",
            )
            .output_key("draft")
            .build()?,
    );`,
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'This works because the runner applies each event’s `state_delta` to the live session as events flow out of the pipeline — by the time the sequential agent starts the writer, both researchers’ keys are in state. See [Sessions and state](/docs/sessions-and-state).',
    },
    { kind: 'h2', text: '4. Add a review loop with exit_loop' },
    {
      kind: 'p',
      text: 'A `LoopAgent` re-runs its children up to `max_iterations` times (the cap must be > 0). To stop early, give the reviewer the built-in [`exit_loop`](/docs/builtin-tools) tool: calling it sets `escalate`, which breaks the loop. Because the reviewer shares `output_key("draft")` with the writer, each rewrite replaces the draft in state.',
    },
    {
      kind: 'code',
      lang: 'rust',
      code: `    let reviewer = Arc::new(
        LlmAgent::builder("reviewer")
            .model(model.clone())
            .instruction(
                "Review this draft against the research in the conversation. If accurate and complete, call the exit_loop tool. Otherwise reply with a corrected rewrite.\\n\\nDraft:\\n{draft}",
            )
            .output_key("draft")
            .tool(adk_rs::tools::exit_loop())
            .build()?,
    );

    let refine = Arc::new(LoopAgent::new(
        "refine",
        "Reviews and refines the draft until it passes",
        vec![reviewer],
        3, // max_iterations: hard cap even if exit_loop is never called
    )?);`,
    },
    { kind: 'h2', text: '5. Wrap the stages in SequentialAgent' },
    {
      kind: 'code',
      lang: 'rust',
      code: `    SequentialAgent::new(
        "research_pipeline",
        "Researches a topic in parallel, drafts a brief, then refines it",
        vec![researchers, writer, refine],
    )
    .map(Arc::new)
}`,
    },
    {
      kind: 'callout',
      tone: 'warn',
      text: 'Escalation does not stop at the loop: `SequentialAgent` also stops when it sees an event with `actions.escalate == Some(true)`. Place a `LoopAgent` that uses `exit_loop` as the **last** step of a sequence, or expect the remaining steps to be skipped.',
    },
    { kind: 'h2', text: '6. Run it and attribute events' },
    {
      kind: 'p',
      text: 'A composed agent runs exactly like a single one: hand the root to a [`Runner`](/docs/runner) and consume the stream. Use `event.author` (the emitting agent’s name) and `event.branch` (set by `ParallelAgent`) to attribute output — parallel events interleave in completion order, so never rely on stream position. Set `GOOGLE_API_KEY` and run:',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'src/main.rs (run)',
      code: `use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let model: Arc<dyn Model> = Arc::new(Gemini::from_env("gemini-2.5-flash")?);
    let runner = Runner::builder()
        .app_name("research")
        .agent(build_pipeline(model)?)
        .session_service(Arc::new(InMemorySessionService::new()))
        .build()?;

    let mut events = runner.run("user", None, "Rust async runtimes in 2026").await?;
    while let Some(ev) = events.next().await {
        let ev = ev?;
        let who = match ev.branch.as_deref() {
            Some(branch) => format!("{} ({branch})", ev.author),
            None => ev.author.clone(),
        };
        if let Some(c) = ev.response.content.as_ref() {
            let text = c.text_concat();
            if !text.is_empty() {
                println!("[{who}]\\n{text}\\n");
            }
        }
    }
    Ok(())
}`,
    },
    {
      kind: 'p',
      text: 'You will see `[tech_researcher (researchers.0)]` and `[market_researcher (researchers.1)]` interleave, then `[writer]`, then `[reviewer]` — ending with either a sign-off `exit_loop` call or a final rewrite. The finished brief also survives the run in session state under `draft`.',
    },
    { kind: 'h2', text: 'Where next' },
    {
      kind: 'list',
      items: [
        '[Workflow agents](/docs/workflow-agents) — full `Sequential`/`Parallel`/`Loop` semantics.',
        '[Multi-agent patterns](/docs/multi-agent) — LLM-driven transfer with `sub_agents` as an alternative to fixed pipelines.',
        '[Guide: Persistent sessions](/docs/guides/persistent-sessions) — keep `draft` and friends across restarts.',
        '[Cancellation and resume](/docs/cancellation-and-resume) — pausing and resuming pipelines with checkpoints.',
      ],
    },
  ],
};
