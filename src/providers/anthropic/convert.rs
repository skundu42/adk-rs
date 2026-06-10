//! Conversion between adk-rs types and the Anthropic Messages API wire format.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::{LlmRequest, LlmResponse};
use crate::error::{ProviderError, Result};
use crate::genai_types::{
    Content, FinishReason, FunctionCall, FunctionDeclaration, Part, Role, UsageMetadata,
};

#[derive(Debug, Serialize)]
pub(crate) struct WireRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    /// Plain string, or an array of text blocks when a `cache_control`
    /// breakpoint is attached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Value>,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    /// Structured-output constraint (`{"format": {"type": "json_schema", ...}}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<Value>,
    /// Extended thinking (`{"type": "enabled", "budget_tokens": N}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WireTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WireMessage {
    pub role: &'static str,
    pub content: Vec<WireBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WireBlock {
    Text {
        text: String,
    },
    Image {
        source: Value,
    },
    Document {
        source: Value,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<WireToolResultPart>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WireToolResultPart {
    Text { text: String },
}

pub(crate) fn to_wire<'a>(req: &'a LlmRequest, model: &'a str) -> WireRequest<'a> {
    // Prompt caching: a `ContextCacheConfig` on the request becomes a
    // `cache_control` breakpoint on the last stable-prefix block. Tools
    // render before system on Anthropic's side, so a breakpoint on the
    // system block caches both; with no system instruction it goes on the
    // last tool instead.
    let cache_control = req.cache_config.as_ref().map(|cfg| {
        if cfg.ttl_seconds >= 3600 {
            serde_json::json!({ "type": "ephemeral", "ttl": "1h" })
        } else {
            serde_json::json!({ "type": "ephemeral" })
        }
    });

    let system_text = req
        .config
        .system_instruction
        .as_ref()
        .map(crate::genai_types::Content::text_concat)
        .filter(|t| !t.is_empty());
    let system = match (&system_text, &cache_control) {
        (Some(text), Some(cc)) => Some(serde_json::json!([
            { "type": "text", "text": text, "cache_control": cc }
        ])),
        (Some(text), None) => Some(Value::String(text.clone())),
        (None, _) => None,
    };

    let mut messages = Vec::with_capacity(req.contents.len());
    for c in &req.contents {
        let (role, blocks) = content_to_blocks(c);
        if !blocks.is_empty() {
            messages.push(WireMessage {
                role,
                content: blocks,
            });
        }
    }

    let mut tools: Vec<WireTool> = req
        .config
        .tools
        .iter()
        .filter_map(|t| match t {
            crate::genai_types::Tool::FunctionDeclarations(decls) => Some(decls),
            _ => None,
        })
        .flat_map(|decls| decls.iter().map(declaration_to_wire))
        .collect();
    // No system block to carry the breakpoint: put it on the last tool.
    if system_text.is_none() {
        if let (Some(cc), Some(last)) = (&cache_control, tools.last_mut()) {
            last.cache_control = Some(cc.clone());
        }
    }

    // Structured outputs: map the request schema to the Messages API's
    // native `output_config.format` (json_schema). The server then enforces
    // the shape, mirroring Gemini's `responseSchema`. Requires a
    // structured-outputs-capable Claude model.
    let output_config = match (&req.config.response_mime_type, &req.config.response_schema) {
        (Some(m), Some(schema)) if m == "application/json" => Some(serde_json::json!({
            "format": {
                "type": "json_schema",
                "schema": crate::providers::common::to_json_schema(schema, false),
            },
        })),
        _ => None,
    };

    // Extended thinking: the unified `thinking_config` budget maps to the
    // Messages API `thinking` parameter. Budget `0` means explicitly
    // disabled; `None` means model default (omit the parameter).
    let thinking_budget = req
        .config
        .thinking_config
        .as_ref()
        .and_then(|tc| tc.thinking_budget)
        .filter(|b| *b > 0)
        .map(|b| b as u32);
    let thinking =
        thinking_budget.map(|b| serde_json::json!({ "type": "enabled", "budget_tokens": b }));

    // `max_tokens` must exceed the thinking budget (thinking tokens count
    // against it), so the *default* grows with the budget. An explicit
    // caller value is always respected.
    let default_max_tokens = thinking_budget.map_or(2048, |b| b + 2048);
    // The API rejects sampling modifications alongside thinking.
    let sampling_allowed = thinking.is_none();
    if !sampling_allowed
        && (req.config.temperature.is_some()
            || req.config.top_p.is_some()
            || req.config.top_k.is_some())
    {
        tracing::debug!(
            "dropping temperature/top_p/top_k: not compatible with Anthropic extended thinking"
        );
    }

    WireRequest {
        model,
        max_tokens: req.config.max_output_tokens.unwrap_or(default_max_tokens),
        system,
        messages,
        tools,
        temperature: req.config.temperature.filter(|_| sampling_allowed),
        top_p: req.config.top_p.filter(|_| sampling_allowed),
        top_k: req.config.top_k.filter(|_| sampling_allowed),
        stop_sequences: req.config.stop_sequences.clone(),
        stream: false,
        output_config,
        thinking,
    }
}

fn declaration_to_wire(d: &FunctionDeclaration) -> WireTool {
    let schema = d
        .parameters
        .as_ref()
        .map(|s| serde_json::to_value(s).unwrap_or_else(|_| serde_json::json!({"type": "OBJECT"})))
        .unwrap_or_else(|| serde_json::json!({"type": "OBJECT"}));
    let schema = lowercase_type_recursive(schema);
    WireTool {
        name: d.name.clone(),
        description: d.description.clone(),
        input_schema: schema,
        cache_control: None,
    }
}

/// Anthropic JSON Schema uses lowercase types; Gemini's are uppercase. Recurse
/// and convert.
fn lowercase_type_recursive(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        if let Some(t) = obj.get_mut("type") {
            if let Some(s) = t.as_str() {
                *t = Value::String(s.to_lowercase());
            }
        }
        for (_, val) in obj.iter_mut() {
            *val = lowercase_type_recursive(val.take());
        }
    } else if let Some(arr) = v.as_array_mut() {
        for val in arr.iter_mut() {
            *val = lowercase_type_recursive(val.take());
        }
    }
    v
}

fn content_to_blocks(c: &Content) -> (&'static str, Vec<WireBlock>) {
    let role = match c.role {
        Role::User | Role::Tool | Role::System => "user",
        Role::Model => "assistant",
    };
    let mut out = Vec::new();
    for p in &c.parts {
        match p {
            Part::Text(t) if !t.is_empty() => {
                out.push(WireBlock::Text { text: t.clone() });
            }
            // Thinking blocks must round-trip verbatim with their signature
            // (the API validates it). An unsigned thought (e.g. produced by
            // a different provider) can't be replayed as thinking and must
            // NOT be downgraded to text — that would misrepresent reasoning
            // as assistant prose — so it is skipped.
            Part::Thought(t) => match &t.signature {
                Some(sig) => out.push(WireBlock::Thinking {
                    thinking: t.text.clone(),
                    signature: sig.clone(),
                }),
                None => tracing::debug!(
                    "skipping unsigned thought part (Anthropic requires signed thinking blocks)"
                ),
            },
            Part::RedactedThought(data) => {
                out.push(WireBlock::RedactedThinking { data: data.clone() })
            }
            Part::FunctionCall(fc) => out.push(WireBlock::ToolUse {
                id: fc
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("call_{}", random_id())),
                name: fc.name.clone(),
                input: fc.args.clone(),
            }),
            Part::FunctionResponse(fr) => out.push(WireBlock::ToolResult {
                tool_use_id: fr.id.clone().unwrap_or_default(),
                content: vec![WireToolResultPart::Text {
                    text: serde_json::to_string(&fr.response).unwrap_or_default(),
                }],
                is_error: false,
            }),
            // Inline binary data: images and PDFs map to base64 sources.
            Part::InlineData(d) if d.mime_type.starts_with("image/") => {
                out.push(WireBlock::Image {
                    source: serde_json::json!({
                        "type": "base64",
                        "media_type": d.mime_type,
                        "data": d.data,
                    }),
                });
            }
            Part::InlineData(d) if d.mime_type == "application/pdf" => {
                out.push(WireBlock::Document {
                    source: serde_json::json!({
                        "type": "base64",
                        "media_type": d.mime_type,
                        "data": d.data,
                    }),
                });
            }
            // File references: https URLs map to url sources.
            Part::FileData(f) if f.file_uri.starts_with("https://") => {
                let source = serde_json::json!({ "type": "url", "url": f.file_uri });
                if f.mime_type.starts_with("image/") {
                    out.push(WireBlock::Image { source });
                } else {
                    out.push(WireBlock::Document { source });
                }
            }
            // Empty text parts fell through the guard above: skip.
            Part::Text(_) => {}
            // Anything else is dropped loudly, not silently.
            other => tracing::warn!(
                part = %part_kind(other),
                "dropping part unsupported by the Anthropic Messages API"
            ),
        }
    }
    (role, out)
}

fn part_kind(p: &Part) -> &'static str {
    match p {
        Part::Text(_) => "text",
        Part::Thought(_) => "thought",
        Part::RedactedThought(_) => "redacted_thought",
        Part::InlineData(_) => "inline_data",
        Part::FileData(_) => "file_data",
        Part::FunctionCall(_) => "function_call",
        Part::FunctionResponse(_) => "function_response",
        _ => "other",
    }
}

fn random_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Anthropic response shape.
#[derive(Debug, Deserialize)]
pub(crate) struct WireResponse {
    pub content: Vec<MaybeKnownBlock>,
    pub stop_reason: Option<String>,
    pub model: Option<String>,
    pub usage: Option<WireUsage>,
}

/// Tolerant wrapper: content blocks the crate doesn't model yet
/// (`redacted_thinking`, `server_tool_use`, future kinds) deserialize into
/// `Unknown` and are skipped, instead of failing the whole response.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum MaybeKnownBlock {
    Known(WireResponseBlock),
    Unknown(Value),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WireResponseBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
}

pub(crate) fn parse_response(body: &[u8]) -> Result<LlmResponse> {
    let r: WireResponse = serde_json::from_slice(body)
        .map_err(|e| ProviderError::Decode(format!("anthropic response: {e}")))?;
    Ok(from_wire_response(r))
}

/// Stop-reason mapping shared by the streaming and non-streaming paths.
pub(crate) fn map_stop_reason(s: &str) -> FinishReason {
    match s {
        "max_tokens" => FinishReason::MaxTokens,
        "refusal" => FinishReason::Safety,
        // end_turn, stop_sequence, tool_use, pause_turn, future values.
        _ => FinishReason::Stop,
    }
}

pub(crate) fn from_wire_response(r: WireResponse) -> LlmResponse {
    let mut parts: Vec<Part> = Vec::with_capacity(r.content.len());
    for b in r.content {
        match b {
            MaybeKnownBlock::Known(WireResponseBlock::Text { text }) => {
                parts.push(Part::Text(text))
            }
            MaybeKnownBlock::Known(WireResponseBlock::Thinking {
                thinking,
                signature,
            }) => parts.push(Part::Thought(crate::genai_types::part::Thought {
                text: thinking,
                signature,
            })),
            MaybeKnownBlock::Known(WireResponseBlock::RedactedThinking { data }) => {
                parts.push(Part::RedactedThought(data))
            }
            MaybeKnownBlock::Known(WireResponseBlock::ToolUse { id, name, input }) => {
                parts.push(Part::FunctionCall(FunctionCall {
                    id: Some(id),
                    name,
                    args: input,
                    thought_signature: None,
                }))
            }
            MaybeKnownBlock::Unknown(v) => {
                let block_type = v.get("type").and_then(Value::as_str).unwrap_or("?");
                tracing::debug!(block_type, "skipping unrecognised anthropic content block");
            }
        }
    }
    let finish = r.stop_reason.as_deref().map(map_stop_reason);
    let cache_read = r
        .usage
        .as_ref()
        .and_then(|u| u.cache_read_input_tokens)
        .unwrap_or(0);
    let cache_written = r
        .usage
        .as_ref()
        .and_then(|u| u.cache_creation_input_tokens)
        .unwrap_or(0);
    let cache_metadata =
        (cache_read > 0 || cache_written > 0).then(|| crate::core::cache::CacheMetadata {
            cache_name: "anthropic/prompt-cache".into(),
            cache_hit: cache_read > 0,
        });
    LlmResponse {
        model_version: r.model,
        content: Some(Content {
            role: Role::Model,
            parts,
        }),
        finish_reason: finish,
        usage_metadata: r.usage.map(|u| UsageMetadata {
            prompt_token_count: Some(u.input_tokens),
            candidates_token_count: Some(u.output_tokens),
            total_token_count: Some(u.input_tokens + u.output_tokens),
            cached_content_token_count: u.cache_read_input_tokens.filter(|n| *n > 0),
            ..UsageMetadata::default()
        }),
        cache_metadata,
        ..LlmResponse::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_types::{FunctionCall, Tool};
    use serde_json::json;

    #[test]
    fn to_wire_collects_assistant_and_user_messages() {
        let mut req = LlmRequest::default();
        req.contents.push(Content::user_text("hi"));
        req.contents.push(Content::model_text("hello!"));
        let w = to_wire(&req, "claude-3-5-sonnet");
        assert_eq!(w.messages.len(), 2);
        assert_eq!(w.messages[0].role, "user");
        assert_eq!(w.messages[1].role, "assistant");
    }

    #[test]
    fn function_call_round_trips_to_tool_use() {
        let mut req = LlmRequest::default();
        req.contents.push(Content {
            role: Role::Model,
            parts: vec![Part::FunctionCall(
                FunctionCall::new("get_weather", json!({"city": "Paris"})).with_id("call-1"),
            )],
        });
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        assert_eq!(w["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(w["messages"][0]["content"][0]["name"], "get_weather");
    }

    #[test]
    fn parse_text_response() {
        let body = json!({
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "model": "claude-3-5-sonnet",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        assert_eq!(r.content.unwrap().text_concat(), "ok");
        assert_eq!(r.finish_reason, Some(FinishReason::Stop));
    }

    /// Regression: unknown content-block types (e.g. `server_tool_use`, or
    /// anything the API adds later) must be skipped, not fail the whole
    /// response.
    #[test]
    fn parse_response_skips_unknown_block_types() {
        let body = json!({
            "content": [
                {"type": "server_tool_use", "id": "x", "name": "web_search", "input": {}},
                {"type": "text", "text": "ok"},
                {"type": "some_future_block", "payload": {"a": 1}}
            ],
            "stop_reason": "end_turn",
            "model": "claude-x",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        let content = r.content.unwrap();
        assert_eq!(content.text_concat(), "ok");
        assert_eq!(content.parts.len(), 1);
    }

    /// Thinking blocks parse with their signature, and `redacted_thinking`
    /// blocks are preserved as opaque parts — both must be replayable.
    #[test]
    fn parse_response_captures_thinking_signature_and_redacted() {
        let body = json!({
            "content": [
                {"type": "thinking", "thinking": "Let me reason…", "signature": "sig-1"},
                {"type": "redacted_thinking", "data": "opaque-blob"},
                {"type": "text", "text": "answer"}
            ],
            "stop_reason": "end_turn"
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        let parts = r.content.unwrap().parts;
        assert_eq!(
            parts[0],
            Part::Thought(
                crate::genai_types::Thought::new("Let me reason…").with_signature("sig-1")
            )
        );
        assert_eq!(parts[1], Part::RedactedThought("opaque-blob".into()));
        assert_eq!(parts[2], Part::Text("answer".into()));
    }

    /// `thinking_config` maps to the `thinking` request parameter; the
    /// default `max_tokens` grows past the budget, and sampling knobs are
    /// dropped (the API rejects them alongside thinking).
    #[test]
    fn thinking_config_maps_to_thinking_param() {
        let mut req = LlmRequest::default();
        req.contents.push(Content::user_text("hi"));
        req.config.thinking_config = Some(crate::genai_types::ThinkingConfig {
            thinking_budget: Some(4096),
            include_thoughts: Some(true),
        });
        req.config.temperature = Some(0.3);
        let w = serde_json::to_value(to_wire(&req, "claude-x")).unwrap();
        assert_eq!(w["thinking"]["type"], "enabled");
        assert_eq!(w["thinking"]["budget_tokens"], 4096);
        assert_eq!(w["max_tokens"], 4096 + 2048);
        assert!(w.get("temperature").is_none());

        // No thinking config → no thinking param, sampling preserved.
        let mut req = LlmRequest::default();
        req.contents.push(Content::user_text("hi"));
        req.config.temperature = Some(0.3);
        let w = serde_json::to_value(to_wire(&req, "claude-x")).unwrap();
        assert!(w.get("thinking").is_none());
        let temp = w["temperature"].as_f64().unwrap();
        assert!((temp - 0.3).abs() < 1e-6, "got {temp}");
    }

    /// Replayed history: signed thoughts become verbatim `thinking` blocks,
    /// redacted thoughts become `redacted_thinking`, and unsigned thoughts
    /// are skipped (never downgraded to assistant prose).
    #[test]
    fn thought_history_round_trips_as_thinking_blocks() {
        let mut req = LlmRequest::default();
        req.contents.push(Content {
            role: Role::Model,
            parts: vec![
                Part::Thought(crate::genai_types::Thought::new("step 1").with_signature("sig-1")),
                Part::RedactedThought("blob".into()),
                Part::thought("unsigned — must be skipped"),
                Part::Text("visible answer".into()),
            ],
        });
        let w = serde_json::to_value(to_wire(&req, "claude-x")).unwrap();
        let blocks = w["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "step 1");
        assert_eq!(blocks[0]["signature"], "sig-1");
        assert_eq!(blocks[1]["type"], "redacted_thinking");
        assert_eq!(blocks[1]["data"], "blob");
        assert_eq!(blocks[2]["type"], "text");
        assert_eq!(blocks[2]["text"], "visible answer");
    }

    /// `refusal` maps to `Safety` on the non-streaming path too (it already
    /// did on the streaming path); unknown stop reasons degrade to `Stop`
    /// rather than `None`.
    #[test]
    fn parse_response_maps_refusal_and_unknown_stop_reasons() {
        let body = json!({
            "content": [{"type": "text", "text": "no"}],
            "stop_reason": "refusal"
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        assert_eq!(r.finish_reason, Some(FinishReason::Safety));

        let body = json!({
            "content": [{"type": "text", "text": "…"}],
            "stop_reason": "pause_turn"
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        assert_eq!(r.finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn parse_tool_use_response() {
        let body = json!({
            "content": [{"type": "tool_use", "id": "tu-1", "name": "f", "input": {"x": 1}}],
            "stop_reason": "tool_use"
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        let calls = r.function_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id.as_deref(), Some("tu-1"));
    }

    #[test]
    fn inline_image_maps_to_image_block() {
        use crate::genai_types::part::InlineData;
        let mut req = LlmRequest::default();
        req.contents.push(Content {
            role: Role::User,
            parts: vec![
                Part::Text("what is this?".into()),
                Part::InlineData(InlineData::from_bytes("image/png", b"px")),
            ],
        });
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        let blocks = w["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "base64");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
    }

    #[test]
    fn inline_pdf_maps_to_document_block() {
        use crate::genai_types::part::InlineData;
        let mut req = LlmRequest::default();
        req.contents.push(Content {
            role: Role::User,
            parts: vec![Part::InlineData(InlineData::from_bytes(
                "application/pdf",
                b"%PDF",
            ))],
        });
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        assert_eq!(w["messages"][0]["content"][0]["type"], "document");
    }

    #[test]
    fn https_file_maps_to_url_source() {
        use crate::genai_types::part::FileData;
        let mut req = LlmRequest::default();
        req.contents.push(Content {
            role: Role::User,
            parts: vec![Part::FileData(FileData {
                mime_type: "image/jpeg".into(),
                file_uri: "https://example.com/cat.jpg".into(),
                display_name: None,
            })],
        });
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        let block = &w["messages"][0]["content"][0];
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "url");
        assert_eq!(block["source"]["url"], "https://example.com/cat.jpg");
    }

    #[test]
    fn cache_config_adds_breakpoint_on_system() {
        use crate::core::ContextCacheConfig;
        let mut req = LlmRequest::default();
        req.append_system_text("a long stable instruction");
        req.cache_config = Some(ContextCacheConfig::default());
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        let sys = w["system"].as_array().unwrap();
        assert_eq!(sys[0]["type"], "text");
        assert_eq!(sys[0]["cache_control"]["type"], "ephemeral");
        assert!(sys[0]["cache_control"].get("ttl").is_none());
    }

    #[test]
    fn cache_config_long_ttl_uses_one_hour() {
        use crate::core::ContextCacheConfig;
        let mut req = LlmRequest::default();
        req.append_system_text("stable");
        req.cache_config = Some(ContextCacheConfig {
            ttl_seconds: 7200,
            ..ContextCacheConfig::default()
        });
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        assert_eq!(w["system"][0]["cache_control"]["ttl"], "1h");
    }

    #[test]
    fn cache_config_without_system_lands_on_last_tool() {
        use crate::core::ContextCacheConfig;
        let mut req = LlmRequest::default();
        req.config.tools.push(Tool::FunctionDeclarations(vec![
            FunctionDeclaration::new("a", ""),
            FunctionDeclaration::new("b", ""),
        ]));
        req.cache_config = Some(ContextCacheConfig::default());
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        assert!(w["tools"][0].get("cache_control").is_none());
        assert_eq!(w["tools"][1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn cache_usage_maps_to_cache_metadata() {
        let body = json!({
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 2,
                "cache_read_input_tokens": 8
            }
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        let meta = r.cache_metadata.unwrap();
        assert!(meta.cache_hit);
        assert_eq!(
            r.usage_metadata.unwrap().cached_content_token_count,
            Some(8)
        );
    }

    #[test]
    fn no_cache_config_keeps_plain_string_system() {
        let mut req = LlmRequest::default();
        req.append_system_text("be brief");
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        assert_eq!(w["system"], "be brief");
    }

    #[test]
    fn response_schema_maps_to_output_config() {
        use crate::genai_types::Schema;
        let mut req = LlmRequest::default();
        req.set_output_schema(
            Schema::object()
                .property("capital", Schema::string())
                .require("capital"),
        );
        let w = serde_json::to_value(to_wire(&req, "claude-sonnet-4-6")).unwrap();
        let format = &w["output_config"]["format"];
        assert_eq!(format["type"], "json_schema");
        assert_eq!(format["schema"]["type"], "object");
        assert_eq!(format["schema"]["additionalProperties"], false);
        assert_eq!(format["schema"]["required"], json!(["capital"]));
        // Original `required` is preserved (no strict all-required rewrite).
        assert_eq!(format["schema"]["properties"]["capital"]["type"], "string");
    }

    #[test]
    fn no_output_config_without_schema() {
        let req = LlmRequest::default();
        let w = serde_json::to_value(to_wire(&req, "claude-sonnet-4-6")).unwrap();
        assert!(w.get("output_config").is_none());
    }

    #[test]
    fn lowercase_type_recursion() {
        let v = json!({"type": "OBJECT", "properties": {"x": {"type": "STRING"}}});
        let v = lowercase_type_recursive(v);
        assert_eq!(v["type"], "object");
        assert_eq!(v["properties"]["x"]["type"], "string");
    }

    #[test]
    fn function_decls_are_lowercased_in_input_schema() {
        let mut req = LlmRequest::default();
        req.config.tools.push(Tool::FunctionDeclarations(vec![
            FunctionDeclaration::new("f", "").with_parameters(crate::genai_types::Schema::object()),
        ]));
        let w = serde_json::to_value(to_wire(&req, "x")).unwrap();
        assert_eq!(w["tools"][0]["input_schema"]["type"], "object");
    }
}
