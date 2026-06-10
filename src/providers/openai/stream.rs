//! SSE decoding for OpenAI-compatible `chat/completions` streaming.
//!
//! Chat-completions chunks carry deltas: incremental `content` strings and
//! fragment-wise `tool_calls` keyed by index (the id and name arrive on the
//! first fragment, the argument JSON accumulates across the rest). We map
//! them onto the crate's chunk contract: text deltas stream immediately as
//! partial [`LlmResponse`] chunks; tool calls and the `finish_reason` /
//! usage are emitted in one final chunk when the stream ends (`[DONE]`).

use std::collections::BTreeMap;

use futures::stream::StreamExt;
use serde_json::Value;

use crate::core::LlmResponse;
use crate::core::stream::LlmResponseStream;
use crate::error::{Error, ProviderError};
use crate::genai_types::{Content, FinishReason, FunctionCall, Part, Role, UsageMetadata};

#[derive(Debug, Default)]
struct ToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

/// Convert a streaming chat-completions response into an
/// [`LlmResponseStream`].
pub(crate) fn from_sse(resp: reqwest::Response) -> LlmResponseStream {
    use eventsource_stream::Eventsource;
    let bytes = resp
        .bytes_stream()
        .map(|r| r.map_err(|e| std::io::Error::other(e.to_string())));
    let mut events = bytes.eventsource();

    let stream = async_stream::try_stream! {
        let mut tools: BTreeMap<u64, ToolCallAccum> = BTreeMap::new();
        let mut finish: Option<FinishReason> = None;
        let mut usage: Option<UsageMetadata> = None;
        let mut model_version: Option<String> = None;

        while let Some(ev) = events.next().await {
            let ev = ev.map_err(|e| Error::Provider(ProviderError::Stream(e.to_string())))?;
            let data = ev.data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                break;
            }
            let v: Value = serde_json::from_str(data)
                .map_err(|e| Error::Provider(ProviderError::Decode(format!("openai sse: {e}"))))?;
            if model_version.is_none() {
                model_version = v.get("model").and_then(Value::as_str).map(str::to_string);
            }
            // Usage arrives in a trailing chunk when stream_options
            // requested it (choices is empty there).
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                let prompt = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0) as u32;
                let completion =
                    u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0) as u32;
                usage = Some(UsageMetadata {
                    prompt_token_count: Some(prompt),
                    candidates_token_count: Some(completion),
                    total_token_count: Some(prompt + completion),
                    ..UsageMetadata::default()
                });
            }
            let Some(choice) = v.get("choices").and_then(Value::as_array).and_then(|c| c.first())
            else {
                continue;
            };
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                finish = Some(match reason {
                    "length" => FinishReason::MaxTokens,
                    "content_filter" => FinishReason::Safety,
                    // stop, tool_calls
                    _ => FinishReason::Stop,
                });
            }
            let delta = &choice["delta"];
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    yield LlmResponse {
                        content: Some(Content {
                            role: Role::Model,
                            parts: vec![Part::Text(text.to_string())],
                        }),
                        ..LlmResponse::default()
                    };
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for frag in calls {
                    let index = frag.get("index").and_then(Value::as_u64).unwrap_or(0);
                    let acc = tools.entry(index).or_default();
                    if let Some(id) = frag.get("id").and_then(Value::as_str) {
                        acc.id = id.to_string();
                    }
                    if let Some(f) = frag.get("function") {
                        if let Some(name) = f.get("name").and_then(Value::as_str) {
                            acc.name.push_str(name);
                        }
                        if let Some(args) = f.get("arguments").and_then(Value::as_str) {
                            acc.arguments.push_str(args);
                        }
                    }
                }
            }
        }

        // Final chunk: completed tool calls (if any), finish reason, usage.
        let parts: Vec<Part> = tools
            .into_values()
            .map(|acc| {
                let args: Value = serde_json::from_str(&acc.arguments)
                    .unwrap_or(Value::Object(Default::default()));
                Part::FunctionCall(FunctionCall {
                    id: Some(acc.id),
                    name: acc.name,
                    args,
                })
            })
            .collect();
        yield LlmResponse {
            model_version,
            content: (!parts.is_empty()).then(|| Content {
                role: Role::Model,
                parts,
            }),
            finish_reason: Some(finish.unwrap_or(FinishReason::Stop)),
            usage_metadata: usage,
            ..LlmResponse::default()
        };
    };
    Box::pin(stream) as LlmResponseStream
}
