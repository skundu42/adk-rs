//! SSE decoding for the Anthropic Messages streaming API.
//!
//! The Messages stream is a typed event sequence (`message_start`,
//! `content_block_start/delta/stop`, `message_delta`, `message_stop`). We
//! map it onto the crate's chunk contract — the one Gemini streaming
//! established: text and thinking deltas are emitted immediately as partial
//! [`LlmResponse`] chunks, tool calls are emitted complete once their
//! argument JSON has fully streamed, and the final chunk carries the
//! `finish_reason` and accumulated usage.

use std::collections::BTreeMap;

use futures::stream::StreamExt;
use serde_json::Value;

use crate::core::LlmResponse;
use crate::core::stream::LlmResponseStream;
use crate::error::{Error, ProviderError};
use crate::genai_types::{Content, FinishReason, FunctionCall, Part, Role, UsageMetadata};

#[derive(Debug, Default)]
struct ToolUseAccum {
    id: String,
    name: String,
    json: String,
}

fn chunk(parts: Vec<Part>) -> LlmResponse {
    LlmResponse {
        content: Some(Content {
            role: Role::Model,
            parts,
        }),
        ..LlmResponse::default()
    }
}

fn map_stop_reason(s: &str) -> FinishReason {
    match s {
        "max_tokens" => FinishReason::MaxTokens,
        "refusal" => FinishReason::Safety,
        // end_turn, stop_sequence, tool_use, pause_turn
        _ => FinishReason::Stop,
    }
}

/// Convert a streaming Messages response into an [`LlmResponseStream`].
pub(crate) fn from_sse(resp: reqwest::Response) -> LlmResponseStream {
    use eventsource_stream::Eventsource;
    let bytes = resp
        .bytes_stream()
        .map(|r| r.map_err(|e| std::io::Error::other(e.to_string())));
    let mut events = bytes.eventsource();

    let stream = async_stream::try_stream! {
        // index → in-flight tool_use accumulation.
        let mut tools: BTreeMap<u64, ToolUseAccum> = BTreeMap::new();
        let mut usage = UsageMetadata::default();
        let mut model_version: Option<String> = None;
        let mut finish: Option<FinishReason> = None;
        let mut cache_read: u32 = 0;
        let mut cache_written: u32 = 0;

        while let Some(ev) = events.next().await {
            let ev = ev.map_err(|e| Error::Provider(ProviderError::Stream(e.to_string())))?;
            if ev.data.is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(&ev.data)
                .map_err(|e| Error::Provider(ProviderError::Decode(format!("anthropic sse: {e}"))))?;
            match v.get("type").and_then(Value::as_str).unwrap_or_default() {
                "message_start" => {
                    let msg = &v["message"];
                    model_version = msg.get("model").and_then(Value::as_str).map(str::to_string);
                    if let Some(u) = msg.get("usage") {
                        usage.prompt_token_count =
                            u.get("input_tokens").and_then(Value::as_u64).map(|n| n as u32);
                        cache_read = u
                            .get("cache_read_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0) as u32;
                        cache_written = u
                            .get("cache_creation_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0) as u32;
                    }
                }
                "content_block_start" => {
                    let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                    let block = &v["content_block"];
                    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        tools.insert(index, ToolUseAccum {
                            id: block.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                            name: block.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                            json: String::new(),
                        });
                    }
                }
                "content_block_delta" => {
                    let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                    let delta = &v["delta"];
                    match delta.get("type").and_then(Value::as_str).unwrap_or_default() {
                        "text_delta" => {
                            if let Some(t) = delta.get("text").and_then(Value::as_str) {
                                yield chunk(vec![Part::Text(t.to_string())]);
                            }
                        }
                        "thinking_delta" => {
                            if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                                yield chunk(vec![Part::Thought(t.to_string())]);
                            }
                        }
                        "input_json_delta" => {
                            if let (Some(acc), Some(frag)) = (
                                tools.get_mut(&index),
                                delta.get("partial_json").and_then(Value::as_str),
                            ) {
                                acc.json.push_str(frag);
                            }
                        }
                        // signature_delta and future delta kinds: skip.
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                    if let Some(acc) = tools.remove(&index) {
                        let args: Value = if acc.json.trim().is_empty() {
                            Value::Object(Default::default())
                        } else {
                            serde_json::from_str(&acc.json).map_err(|e| {
                                Error::Provider(ProviderError::Decode(format!(
                                    "tool_use arguments: {e}"
                                )))
                            })?
                        };
                        yield chunk(vec![Part::FunctionCall(FunctionCall {
                            id: Some(acc.id),
                            name: acc.name,
                            args,
                        })]);
                    }
                }
                "message_delta" => {
                    if let Some(s) = v["delta"].get("stop_reason").and_then(Value::as_str) {
                        finish = Some(map_stop_reason(s));
                    }
                    if let Some(n) = v["usage"].get("output_tokens").and_then(Value::as_u64) {
                        usage.candidates_token_count = Some(n as u32);
                    }
                }
                "message_stop" => {
                    usage.total_token_count = Some(
                        usage.prompt_token_count.unwrap_or(0)
                            + usage.candidates_token_count.unwrap_or(0),
                    );
                    if cache_read > 0 {
                        usage.cached_content_token_count = Some(cache_read);
                    }
                    let cache_metadata = (cache_read > 0 || cache_written > 0).then(|| {
                        crate::core::cache::CacheMetadata {
                            cache_name: "anthropic/prompt-cache".into(),
                            cache_hit: cache_read > 0,
                        }
                    });
                    yield LlmResponse {
                        model_version: model_version.take(),
                        finish_reason: Some(finish.take().unwrap_or(FinishReason::Stop)),
                        usage_metadata: Some(usage),
                        cache_metadata,
                        ..LlmResponse::default()
                    };
                    break;
                }
                "error" => {
                    let msg = v["error"]["message"].as_str().unwrap_or("unknown stream error");
                    Err(Error::Provider(ProviderError::Stream(msg.to_string())))?;
                }
                // "ping" and unknown event types: skip.
                _ => {}
            }
        }
    };
    Box::pin(stream) as LlmResponseStream
}
