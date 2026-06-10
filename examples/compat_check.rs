//! Live wire-compatibility smoke test against the real Anthropic and
//! OpenAI APIs.
//!
//! Exercises every wire shape adk-rs sends — generation, SSE streaming,
//! tool calling, structured output, image input, and (Anthropic) prompt-
//! cache breakpoints — and prints PASS/FAIL per check. Providers without a
//! key in the environment are skipped.
//!
//! ```sh
//! ANTHROPIC_API_KEY=... OPENAI_API_KEY=... \
//!   cargo run --example compat_check --features "anthropic,openai"
//! ```
//!
//! Override the models with `ANTHROPIC_MODEL` (default `claude-sonnet-4-6`;
//! structured output needs a 4.5-generation-or-newer model) and
//! `OPENAI_MODEL` (default `gpt-4o-mini`).

use std::sync::Arc;

use adk_rs::core::{ContextCacheConfig, LlmRequest, Model};
use adk_rs::genai_types::{
    Content, FunctionDeclaration, Part, Role, Schema, Tool, part::InlineData,
};
use futures::TryStreamExt;

/// 1x1 red PNG.
const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

struct Tally {
    passed: u32,
    failed: u32,
}

impl Tally {
    async fn check<F>(&mut self, label: &str, fut: F)
    where
        F: std::future::Future<Output = adk_rs::Result<String>>,
    {
        match fut.await {
            Ok(detail) => {
                self.passed += 1;
                println!("  PASS  {label}: {detail}");
            }
            Err(e) => {
                self.failed += 1;
                println!("  FAIL  {label}: {e}");
            }
        }
    }
}

fn user(text: &str) -> LlmRequest {
    LlmRequest {
        contents: vec![Content::user_text(text)],
        ..Default::default()
    }
}

fn ok(cond: bool, detail: String, ctx: &str) -> adk_rs::Result<String> {
    if cond {
        Ok(detail)
    } else {
        Err(adk_rs::core::Error::other(format!("{ctx}: {detail}")))
    }
}

async fn check_basic(model: &dyn Model) -> adk_rs::Result<String> {
    let r = model
        .generate_content(user("Reply with the single word: pong"))
        .await?;
    let text = r.content.map(|c| c.text_concat()).unwrap_or_default();
    ok(
        text.to_lowercase().contains("pong"),
        format!("got {text:?}"),
        "expected pong",
    )
}

async fn check_streaming(model: &dyn Model) -> adk_rs::Result<String> {
    let chunks: Vec<_> = model
        .stream_generate_content(user("Count from 1 to 10, separated by commas."))
        .await?
        .try_collect()
        .await?;
    let text: String = chunks
        .iter()
        .filter_map(|c| c.content.as_ref().map(Content::text_concat))
        .collect();
    let finals = chunks.iter().filter(|c| c.finish_reason.is_some()).count();
    ok(
        chunks.len() > 2 && text.contains("10") && finals >= 1,
        format!("{} chunks, finish present: {}", chunks.len(), finals >= 1),
        "expected a multi-chunk stream ending in a finish reason",
    )
}

async fn check_tool_call(model: &dyn Model) -> adk_rs::Result<String> {
    let mut req = user("What is the weather in Paris? Use the get_weather tool.");
    req.config.tools.push(Tool::FunctionDeclarations(vec![
        FunctionDeclaration::new("get_weather", "Get current weather for a city").with_parameters(
            Schema::object()
                .property("city", Schema::string())
                .require("city"),
        ),
    ]));
    let r = model.generate_content(req).await?;
    let calls = r.function_calls();
    ok(
        calls.iter().any(|c| c.name == "get_weather"),
        format!(
            "{} call(s): {:?}",
            calls.len(),
            calls.iter().map(|c| &c.name).collect::<Vec<_>>()
        ),
        "expected a get_weather call",
    )
}

async fn check_structured_output(model: &dyn Model) -> adk_rs::Result<String> {
    let mut req = user("Give me facts about France.");
    req.set_output_schema(
        Schema::object()
            .property("capital", Schema::string())
            .property("population_millions", Schema::number())
            .require("capital")
            .require("population_millions"),
    );
    let r = model.generate_content(req).await?;
    let text = r.content.map(|c| c.text_concat()).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| adk_rs::core::Error::other(format!("response was not JSON: {e}: {text}")))?;
    ok(
        v["capital"].as_str().map(str::to_lowercase) == Some("paris".into()),
        format!("parsed {v}"),
        "expected capital == Paris",
    )
}

async fn check_image_input(model: &dyn Model) -> adk_rs::Result<String> {
    let req = LlmRequest {
        contents: vec![Content {
            role: Role::User,
            parts: vec![
                Part::Text("What colour is this image? One word.".into()),
                Part::InlineData(InlineData {
                    mime_type: "image/png".into(),
                    data: TINY_PNG_B64.into(),
                    display_name: None,
                }),
            ],
        }],
        ..Default::default()
    };
    let r = model.generate_content(req).await?;
    let text = r
        .content
        .map(|c| c.text_concat())
        .unwrap_or_default()
        .to_lowercase();
    ok(
        text.contains("red"),
        format!("got {text:?}"),
        "expected the model to see a red pixel",
    )
}

async fn check_anthropic_cache_shape(model: &dyn Model) -> adk_rs::Result<String> {
    // Verifies the API accepts cache_control breakpoints; whether the prefix
    // actually caches depends on the server-side token minimum.
    let mut req = user("Reply with the single word: pong");
    req.append_system_text(&"You are a precise assistant. ".repeat(50));
    req.cache_config = Some(ContextCacheConfig::default());
    let r = model.generate_content(req).await?;
    let cached = r
        .usage_metadata
        .and_then(|u| u.cached_content_token_count)
        .unwrap_or(0);
    Ok(format!(
        "request accepted; cache_metadata: {:?}, cached tokens: {cached}",
        r.cache_metadata.map(|m| m.cache_hit)
    ))
}

async fn run_provider(name: &str, model: Arc<dyn Model>, anthropic: bool, tally: &mut Tally) {
    println!("\n== {name} ({}) ==", model.name());
    tally.check("basic generation", check_basic(&*model)).await;
    tally.check("sse streaming", check_streaming(&*model)).await;
    tally.check("tool calling", check_tool_call(&*model)).await;
    tally
        .check("structured output", check_structured_output(&*model))
        .await;
    tally.check("image input", check_image_input(&*model)).await;
    if anthropic {
        tally
            .check(
                "prompt-cache breakpoints",
                check_anthropic_cache_shape(&*model),
            )
            .await;
    }
}

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let mut tally = Tally {
        passed: 0,
        failed: 0,
    };
    let mut ran = false;

    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        ran = true;
        let m = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-sonnet-4-6".into());
        let model = Arc::new(adk_rs::providers::anthropic::Anthropic::from_env(m)?);
        run_provider("Anthropic", model, true, &mut tally).await;
    } else {
        println!("ANTHROPIC_API_KEY not set — skipping Anthropic checks");
    }

    if std::env::var("OPENAI_API_KEY").is_ok() {
        ran = true;
        let m = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        let model = Arc::new(adk_rs::providers::openai::OpenAi::from_env(m)?);
        run_provider("OpenAI", model, false, &mut tally).await;
    } else {
        println!("OPENAI_API_KEY not set — skipping OpenAI checks");
    }

    if !ran {
        println!("\nNo provider keys found; nothing was checked.");
        return Ok(());
    }
    println!("\n{} passed, {} failed", tally.passed, tally.failed);
    if tally.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
