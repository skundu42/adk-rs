import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'function-tools',
  title: 'Function tools & #[tool]',
  description:
    'Turning plain async Rust functions into agent tools, either with the #[tool] proc-macro or the explicit FunctionTool and LongRunningFunctionTool wrappers.',
  srcPath: 'src/tools/function_tool.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Most custom tools are just an async function. The `#[tool]` attribute macro turns one into a fully-declared `Arc<dyn Tool>` — schema derived from your args struct, description lifted from your doc comment. When you need runtime-built names, schemas, or confirmation flags, drop down to `FunctionTool`.',
    },
    { kind: 'h2', text: 'The #[tool] macro' },
    {
      kind: 'p',
      text: 'The macro lives in the sibling `adk-rs-macros` crate and is re-exported as `adk_rs::tool` (and `adk_rs::tools::tool`) behind the `macros` feature. It accepts an async function with **exactly two parameters**: a typed args struct and `&mut ToolContext`.',
    },
    {
      kind: 'list',
      items: [
        'The function must be `async fn` — the macro rejects sync functions at compile time.',
        'The first parameter is your args struct; it must implement `serde::Deserialize` **and** `schemars::JsonSchema`. Doc comments on its fields flow into the generated parameter schema.',
        'The second parameter must be `&mut ToolContext`. Receivers (`self`) are not supported.',
        'The return type is `adk_rs::Result<T>` where `T: serde::Serialize` — the value is serialized to JSON for the `FunctionResponse`.',
        'The doc comment on the function becomes the tool’s `description()` (lines are joined with newlines).',
      ],
    },
    { kind: 'h3', text: 'What it generates' },
    {
      kind: 'p',
      text: 'For `async fn get_weather(...)`, the macro emits a PascalCase unit struct `GetWeather`, a `DynTool` impl on it, and a free constructor function `get_weather() -> Arc<dyn DynTool>` that shadows the original fn name. The impl: `name()` returns `"get_weather"`; `declaration()` builds a `FunctionDeclaration` whose parameters come from `schemars::schema_for!(Args)` converted through `Schema::from_schemars`; `run()` deserializes the JSON args into your struct — returning `ToolError::InvalidArgs` with a useful message on mismatch — awaits your body, and serializes the result.',
    },
    { kind: 'h2', text: 'Full example: a weather tool' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'examples/weather_agent.rs (condensed)',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::core::ToolContext;
use adk_rs::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

/// Look up the current weather in \`args.city\`.
#[tool]
async fn get_weather(
    args: GetWeatherArgs,
    _ctx: &mut ToolContext,
) -> adk_rs::Result<WeatherReport> {
    Ok(WeatherReport {
        city: args.city,
        temp_c: 22.0,
        description: "sunny".into(),
    })
}

let agent = LlmAgent::builder("weather")
    .model(model)
    .instruction("Use get_weather to answer questions about cities' weather.")
    .tool(get_weather()) // the generated constructor
    .build()?;`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'Bad arguments never reach your function body. If the model sends `{"msg": 42}` where a string is expected, `run` fails with `ToolError::InvalidArgs { tool, message }` and the error text is fed back to the model, which usually self-corrects on the next iteration.',
    },
    { kind: 'h2', text: 'FunctionTool: the explicit fallback' },
    {
      kind: 'p',
      text: '`FunctionTool` wraps an `async fn(Value, &mut ToolContext) -> Result<Value>` with explicitly supplied metadata. Use it when names or schemas are only known at runtime, or when you need the confirmation/long-running switches the macro does not expose.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'FunctionTool::new(name, description, parameters: Option<Schema>, f: FunctionToolFn) -> Self',
          desc: 'Core constructor taking a boxed-future closure. `parameters: None` advertises an empty object schema.',
        },
        {
          sig: 'FunctionTool::from_async(name, description, parameters, f) -> Self',
          desc: 'Convenience wrapper accepting any `Fn(Value, &mut ToolContext) -> impl Future<Output = Result<Value>>` — no manual boxing.',
        },
        {
          sig: 'fn with_long_running(self, yes: bool) -> Self',
          desc: 'Makes `is_long_running()` return `true`, so calls pause the invocation (see below).',
        },
        {
          sig: 'fn require_confirmation(self, yes: bool) -> Self',
          desc: 'Requires explicit user approval before every call; the agent pauses with an `adk_request_confirmation` request. See [tool confirmation](/docs/tool-confirmation).',
        },
        {
          sig: 'fn with_confirmation_hint(self, hint: impl Into<String>) -> Self',
          desc: 'Custom prompt shown to the user instead of the generic “Approve execution of tool …?”.',
        },
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'A guarded delete tool',
      code: `use adk_rs::core::ToolContext;
use adk_rs::genai_types::Schema;
use adk_rs::tools::FunctionTool;
use serde_json::{Value, json};
use std::sync::Arc;

let delete = FunctionTool::from_async(
    "delete_record",
    "Delete a record by id. Irreversible.",
    Some(Schema::object().property("id", Schema::string()).require("id")),
    |args: Value, _ctx: &mut ToolContext| async move {
        // ... perform the deletion ...
        Ok(json!({"deleted": args["id"]}))
    },
)
.require_confirmation(true)
.with_confirmation_hint("Permanently delete this record?");

let tool: Arc<dyn adk_rs::Tool> = Arc::new(delete);`,
    },
    { kind: 'h2', text: 'LongRunningFunctionTool and will_continue' },
    {
      kind: 'p',
      text: '`LongRunningFunctionTool::new(name, description, schema: Option<Schema>, handler)` returns an `Arc<Self>` whose `is_long_running()` is `true` and whose `run` also sets `ctx.long_running = true`. The handler should return quickly with an operation **handle** (a ticket id, a status URL), not the final result.',
    },
    {
      kind: 'p',
      text: 'The agent then emits the tool’s `FunctionResponse` with `will_continue: Some(true)`, records the call id in the event’s `long_running_tool_ids`, marks the invocation paused, and yields the stream. Your application completes the work out of band and resumes by submitting a fresh `FunctionResponse` carrying the final result on a follow-up invocation — the agent replays it into the conversation and the model continues from there. The same pause/resume mechanics back the built-in `get_user_choice` tool and the confirmation/auth gates. See [cancellation & resume](/docs/cancellation-and-resume).',
    },
    { kind: 'h2', text: 'Macro or manual?' },
    {
      kind: 'table',
      head: ['Situation', 'Reach for'],
      rows: [
        ['Typed args, compile-time name, description from docs', '`#[tool]`'],
        ['Name/description/schema computed at runtime', '`FunctionTool::from_async`'],
        ['Needs `require_confirmation` / `with_confirmation_hint`', '`FunctionTool`'],
        ['Returns an operation handle, completes out of band', '`LongRunningFunctionTool` (or `FunctionTool::with_long_running`)'],
        ['Custom `auth_config`, per-args confirmation, or `process_llm_request` logic', 'implement `DynTool` directly'],
      ],
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Tools overview](/docs/tools-overview) — the `DynTool` trait and `ToolContext` in full.',
        '[Built-in tools](/docs/builtin-tools) — what ships in the box.',
        '[Weather agent example](/docs/examples/weather-agent) — the macro in a complete program.',
      ],
    },
  ],
};
