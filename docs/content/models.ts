import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'models',
  title: 'Models & requests',
  description:
    'The Model trait that abstracts every LLM provider, the LlmRequest and LlmResponse types that travel through it, and the genai_types wire layer underneath.',
  srcPath: 'src/core/model.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Every LLM in adk-rs sits behind one trait: `Model`. Agents build a provider-neutral `LlmRequest`, the model returns a provider-neutral `LlmResponse`, and the wire-level details — Gemini JSON, Anthropic Messages, OpenAI chat completions — stay inside the provider crates. Swap models by swapping one `Arc<dyn Model>`.',
    },
    { kind: 'h2', text: 'The Model trait' },
    {
      kind: 'p',
      text: 'Defined in `src/core/model.rs`, the trait has two required methods and one default. Implementations must be `Send + Sync + Debug + \'static`, so a model handle can be shared freely across agents and tasks.',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn name(&self) -> &str',
          desc: 'Canonical name of this instance, e.g. `"gemini-2.5-flash"`.',
        },
        {
          sig: "fn supported_models(&self) -> &'static [&'static str]",
          desc: 'Glob-like name patterns this provider can serve (e.g. `"gemini-*"`). Used by `ModelRegistry` to dispatch by model name.',
        },
        {
          sig: 'async fn generate_content(&self, req: LlmRequest) -> Result<LlmResponse>',
          desc: 'Single-shot generation. The only method a custom model must really implement.',
        },
        {
          sig: 'async fn stream_generate_content(&self, req: LlmRequest) -> Result<LlmResponseStream>',
          desc: 'Streaming generation. The default implementation calls `generate_content` and wraps the result in a one-element stream, so non-streaming backends work everywhere streaming is expected.',
        },
      ],
    },
    { kind: 'h2', text: 'ModelRegistry' },
    {
      kind: 'p',
      text: '`ModelRegistry` maps model-name patterns to provider instances. `register` indexes a model under its exact `name()` and under every `supported_models()` glob; `get` checks exact names first, then walks the glob patterns in insertion order and returns the first match. There is no global state — registries are plain values.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Dispatching by name',
      code: `use adk_rs::core::{Model, ModelRegistry};
use std::sync::Arc;

let mut registry = ModelRegistry::new();
registry.register(Arc::new(gemini));    // supported_models: ["gemini-*"]
registry.register(Arc::new(claude));    // supported_models: ["claude-*"]

let m: Arc<dyn Model> = registry.get("gemini-2.5-pro").expect("glob match");`,
    },
    { kind: 'h2', text: 'Anatomy of an LlmRequest' },
    {
      kind: 'p',
      text: '`LlmRequest` (in `src/core/llm_request.rs`) bundles everything one model call needs. The system instruction is **not** in `contents` — it lives in `config.system_instruction`.',
    },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Purpose'],
      rows: [
        ['`model`', '`Option<String>`', 'Model identifier, e.g. `gemini-2.5-flash`.'],
        ['`contents`', '`Vec<Content>`', 'The conversation history sent to the model.'],
        ['`config`', '`GenerateContentConfig`', 'System instruction, tool declarations, sampling knobs.'],
        ['`tools_dict`', '`HashMap<String, Arc<dyn DynTool>>`', 'Live tool objects keyed by name, used by the agent to dispatch `FunctionCall`s. Skipped during serialization.'],
        ['`cache_config`', '`Option<ContextCacheConfig>`', 'Opt-in [context caching](/docs/context-caching); cache-capable providers (Gemini) cache the stable prefix server-side, others ignore it.'],
      ],
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn append_system_text(&mut self, text: &str)',
          desc: 'Appends to (or sets) `config.system_instruction`, joining segments with a blank line.',
        },
        {
          sig: 'fn append_function_declarations(&mut self, decls: impl IntoIterator<Item = FunctionDeclaration>)',
          desc: 'Adds tool declarations to `config.tools`, merging into an existing `Tool::FunctionDeclarations` entry when one is present so the wire payload stays a single list.',
        },
        {
          sig: 'fn set_output_schema(&mut self, schema: Schema)',
          desc: 'Sets `config.response_schema` and forces `config.response_mime_type` to `application/json` — the mechanism behind [structured output](/docs/structured-output).',
        },
      ],
    },
    { kind: 'h2', text: 'Anatomy of an LlmResponse' },
    {
      kind: 'p',
      text: '`LlmResponse` is the provider-neutral payload returned by both single-shot calls and each streaming chunk. A response is either content-bearing or error-bearing; `is_error()` checks whether `error_code` is set.',
    },
    {
      kind: 'table',
      head: ['Field', 'Type', 'Purpose'],
      rows: [
        ['`content`', '`Option<Content>`', 'Generated content (text, function calls, code, ...).'],
        ['`finish_reason`', '`Option<FinishReason>`', 'Why generation stopped: `Stop`, `MaxTokens`, `Safety`, `Recitation`, `MalformedFunctionCall`, and friends.'],
        ['`usage_metadata`', '`Option<UsageMetadata>`', 'Token counts: `prompt_token_count`, `candidates_token_count`, `total_token_count`, `cached_content_token_count`, `thoughts_token_count`.'],
        ['`cache_metadata`', '`Option<CacheMetadata>`', 'Cache name and hit/miss flag, set by cache-capable providers when a `ContextCacheConfig` is active.'],
        ['`error_code` / `error_message`', '`Option<String>`', 'Provider-specific error info; populated from non-`Stop` finish reasons or prompt-feedback block reasons.'],
        ['`grounding_metadata` / `citation_metadata`', 'optional', 'Search-grounding chunks and citations (Gemini built-in tools).'],
        ['`model_version` / `interrupted` / `custom_metadata`', 'optional', 'Producing model, mid-stream interruption flag, free-form metadata.'],
      ],
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn from_generate(resp: GenerateContentResponse) -> Self',
          desc: 'Builds an `LlmResponse` from a wire-level response: takes the first candidate; if it has parts or finished with `Stop` it becomes content, otherwise the finish reason becomes `error_code`. Blocked prompts surface `prompt_feedback.block_reason` as the error.',
        },
        { sig: 'fn is_error(&self) -> bool', desc: 'True when `error_code` is set.' },
        {
          sig: 'fn function_calls(&self) -> Vec<FunctionCall>',
          desc: 'Extracts every `Part::FunctionCall` from the content.',
        },
        {
          sig: 'fn function_responses(&self) -> Vec<FunctionResponse>',
          desc: 'Extracts every `Part::FunctionResponse` from the content.',
        },
      ],
    },
    { kind: 'h2', text: 'The genai_types layer' },
    {
      kind: 'p',
      text: 'The `genai_types` module holds the wire-neutral data shapes shared by every provider. A `Content` is a `{role, parts}` pair — `Role` is `User`, `Model`, `System`, or `Tool` — with helpers `Content::user_text`, `Content::model_text`, `Content::system_text`, and `text_concat()` to join all text parts.',
    },
    {
      kind: 'p',
      text: 'The `Part` enum is the unit of content. Gemini discriminates parts by field presence rather than a tag, so `Part` carries hand-written `Serialize`/`Deserialize` impls. The variants:',
    },
    {
      kind: 'list',
      items: [
        '`Part::Text(String)` — plain text. Build with `Part::text("...")`.',
        '`Part::InlineData(InlineData)` — inline base64 binary with a MIME type. Build from raw bytes with `Part::inline_bytes(mime, bytes)`.',
        '`Part::FileData(FileData)` — external file reference (`file_uri` + `mime_type`).',
        '`Part::FunctionCall(FunctionCall)` — a model-emitted tool call.',
        '`Part::FunctionResponse(FunctionResponse)` — a tool-emitted result.',
        '`Part::ExecutableCode(ExecutableCode)` — code the model wants executed (`language`, `code`).',
        '`Part::CodeExecutionResult(CodeExecutionResult)` — execution outcome (`outcome`, `output`). See [code execution](/docs/code-execution).',
        '`Part::Thought(String)` — Gemini reasoning-trace text, serialized as `{"text": ..., "thought": true}`.',
      ],
    },
    { kind: 'h3', text: 'GenerateContentConfig knobs' },
    {
      kind: 'p',
      text: 'Beyond `system_instruction` and `tools`, `GenerateContentConfig` exposes sampling and safety controls: `temperature`, `top_p`, `top_k`, `max_output_tokens`, `candidate_count`, `stop_sequences`, `seed`, `presence_penalty`, `frequency_penalty`; `response_mime_type` + `response_schema` for structured output; `safety_settings` (a list of `SafetySetting { category, threshold }`); `thinking_config` (`ThinkingConfig { thinking_budget, include_thoughts }`); and `tool_config` (`ToolMode::Auto` / `Any` / `None` plus `allowed_function_names`). The default config serializes to an empty JSON object — only what you set goes over the wire.',
    },
    { kind: 'h2', text: 'Implementing a custom Model' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'A canned-response model',
      code: `use adk_rs::core::{LlmRequest, LlmResponse, Model};
use adk_rs::genai_types::Content;
use async_trait::async_trait;

#[derive(Debug)]
struct CannedModel;

#[async_trait]
impl Model for CannedModel {
    fn name(&self) -> &str {
        "canned-1"
    }

    fn supported_models(&self) -> &'static [&'static str] {
        &["canned-*"]
    }

    async fn generate_content(&self, _req: LlmRequest) -> adk_rs::Result<LlmResponse> {
        Ok(LlmResponse {
            content: Some(Content::model_text("Always the same answer.")),
            ..LlmResponse::default()
        })
    }
    // stream_generate_content: default impl wraps this in a 1-element stream.
}`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'A custom `Model` is also the cleanest test double: hand an `LlmAgent` a mock that returns scripted `FunctionCall`s and assert on the resulting event stream. See [Testing](/docs/testing).',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Providers](/docs/providers) — the shipped `Gemini`, `Anthropic`, and `OpenAi` implementations.',
        '[Structured output](/docs/structured-output) — `set_output_schema` end to end.',
        '[Context caching](/docs/context-caching) — `cache_config` and `cache_metadata` in detail.',
        '[Events](/docs/events) — how `LlmResponse` values become session events.',
      ],
    },
  ],
};
