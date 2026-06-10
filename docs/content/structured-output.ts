import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'structured-output',
  title: 'Structured output',
  description:
    'Force an LlmAgent to reply with schema-conforming JSON and capture the parsed result in session state.',
  srcPath: 'src/agents/llm_agent.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Setting `.output_schema(...)` on an `LlmAgent` switches the model into JSON mode: every response must conform to the schema you declare, and with `.output_key(...)` the parsed result is written into session state where downstream agents — or your own code — can read it.',
    },
    { kind: 'h2', text: 'How it works' },
    {
      kind: 'p',
      text: '`LlmAgentBuilder::output_schema` takes a [`Schema`](/docs/agents-overview) from `adk_rs::genai_types` — the OpenAPI-3-flavored schema type the crate uses everywhere (tool declarations, OpenAPI tools, structured output). When the agent builds each `LlmRequest`, it calls `LlmRequest::set_output_schema`, which sets two fields on the request config: `response_schema` to your schema, and `response_mime_type` to `"application/json"`.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'LlmAgentBuilder::output_schema(self, schema: Schema) -> Self',
          desc: 'Force structured JSON output conforming to `schema`.',
        },
        {
          sig: 'LlmAgentBuilder::output_key(self, key: impl Into<String>) -> Self',
          desc: 'Save the agent\'s final response into session state under this key, via the final event\'s `state_delta`. With `output_schema`, the stored value is the parsed JSON value rather than raw text.',
        },
        {
          sig: 'LlmRequest::set_output_schema(&mut self, schema: Schema)',
          desc: 'Sets `config.response_schema = Some(schema)` and `config.response_mime_type = Some("application/json")`.',
        },
      ],
    },
    { kind: 'h2', text: 'Building a Schema' },
    {
      kind: 'p',
      text: '`Schema` has fluent constructors for each JSON type plus `property`/`require` for objects. If you already derive `schemars::JsonSchema` on your structs, `Schema::from_schemars` converts a schemars `RootSchema` (best-effort: `$ref` into `#/definitions` is resolved, but constructs like `oneOf` become `SchemaError`s). The crate depends on `schemars` 0.8.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Hand-built vs derived',
      code: `use adk_rs::genai_types::Schema;

// Hand-built.
let schema = Schema::object()
    .property("capital", Schema::string().with_description("Capital city"))
    .property("population", Schema::integer())
    .require("capital")
    .require("population");

// Derived from a schemars type.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CountryInfo {
    capital: String,
    population: u64,
}
let root = schemars::schema_for!(CountryInfo);
let schema = Schema::from_schemars(&root)?;`,
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'For Gemini, `Schema::sanitize_for_gemini` strips constructs the Gemini proto rejects (`default`, `pattern`, `example`, `title`, recursively) and errors on `enum` values attached to non-string types.',
    },
    { kind: 'h2', text: 'Capturing the result with output_key' },
    {
      kind: 'p',
      text: 'When the agent produces its final response (an event with no function calls left to dispatch), it concatenates the text parts and stamps the value into `event.actions.state_delta` under your key. With `output_schema` set, the text is first parsed with `serde_json::from_str`; if the model somehow returned invalid JSON, the agent logs a warning and stores the raw text as a JSON string instead of failing the run.',
    },
    {
      kind: 'p',
      text: 'Because string instructions are templated against session state, a later agent in a [workflow pipeline](/docs/workflow-agents) can reference the value directly: an instruction like `"Summarize this data: {info}"` has `{info}` replaced with the stored state value when the request is built. See [Sessions & state](/docs/sessions-and-state).',
    },
    { kind: 'h2', text: 'Provider mapping' },
    {
      kind: 'p',
      text: 'All three providers now enforce the schema **server-side**. The adk schema (OpenAPI-3 flavour) is converted to each provider\'s JSON Schema dialect on the way out: types lowercased, `nullable` becoming a `["type", "null"]` union, `additionalProperties: false` added to objects, and advisory keywords the providers reject (`pattern`, `format`, `minimum`, length/item bounds, …) stripped.',
    },
    {
      kind: 'table',
      head: ['Provider', 'What is sent'],
      rows: [
        [
          '[Gemini](/docs/providers)',
          'Native: `responseSchema` and `responseMimeType` go onto the wire request.',
        ],
        [
          'OpenAI-compatible',
          'Strict structured outputs: `response_format: {"type": "json_schema", "json_schema": {"strict": true, ...}}`. Strict mode requires every property listed in `required`, so previously-optional properties are sent as required-but-nullable — the original semantics survive. A JSON mime type *without* a schema still maps to plain `{"type": "json_object"}` mode.',
        ],
        [
          'Anthropic',
          'Native structured outputs: the Messages API\'s `output_config: {"format": {"type": "json_schema", ...}}`. Requires a structured-outputs-capable Claude model (Claude 4.5-generation and newer).',
        ],
      ],
    },
    { kind: 'h2', text: 'Complete example' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'examples/structured.rs',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::genai_types::Schema;
use adk_rs::providers::gemini::Gemini;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;
use futures::StreamExt;
use std::sync::Arc;

#[derive(Debug, serde::Deserialize)]
struct CountryInfo {
    capital: String,
    population: u64,
}

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let schema = Schema::object()
        .property("capital", Schema::string().with_description("Capital city"))
        .property("population", Schema::integer())
        .require("capital")
        .require("population");

    let agent = LlmAgent::builder("country_info")
        .model(Arc::new(Gemini::from_env("gemini-2.5-flash")?))
        .instruction("Answer with facts about the country the user names.")
        .output_schema(schema)
        .output_key("info") // parsed JSON also lands in state["info"]
        .build()?;

    let runner = Runner::builder()
        .app_name("structured")
        .agent(Arc::new(agent))
        .session_service(Arc::new(InMemorySessionService::new()))
        .build()?;

    let mut events = runner.run("user", None, "Tell me about France").await?;
    while let Some(event) = events.next().await {
        let event = event?;
        if event.is_final_response() {
            if let Some(content) = &event.response.content {
                let info: CountryInfo = serde_json::from_str(&content.text_concat())?;
                println!("capital={} population={}", info.capital, info.population);
            }
        }
    }
    Ok(())
}`,
    },
    { kind: 'h2', text: 'Interaction with tools' },
    {
      kind: 'p',
      text: 'The agent applies the schema to *every* request it builds inside its LLM↔tool loop, including requests that also carry tool declarations — adk-rs does not forbid the combination. JSON parsing only happens on the final response (the turn with no function calls). In practice structured output works best on leaf agents whose single job is to emit a record; if you need tool use *and* structured output, consider a two-step [pipeline](/docs/guides/multi-agent-pipeline) where the first agent gathers data with tools and the second formats it.',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[LlmAgent](/docs/llm-agent) — the full builder surface.',
        '[Sessions & state](/docs/sessions-and-state) — how `state_delta` is applied and persisted.',
        '[Workflow agents](/docs/workflow-agents) — chaining agents that read each other\'s `output_key`.',
      ],
    },
  ],
};
