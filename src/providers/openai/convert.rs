//! Conversion between adk-rs and OpenAI's `/v1/chat/completions` wire format.

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
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Successor to `max_tokens`; the only form reasoning models accept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WireMessage {
    pub role: &'static str,
    /// Either a plain string or an array of content parts (text /
    /// image_url) — the chat-completions content union.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall>,
}

/// Build the content union for a user message: a plain string when the
/// content is text-only, an array of `text` / `image_url` parts otherwise.
fn user_content(c: &Content) -> Value {
    let has_media = c.parts.iter().any(|p| {
        matches!(p, Part::InlineData(d) if d.mime_type.starts_with("image/"))
            || matches!(p, Part::FileData(f)
                if f.file_uri.starts_with("https://") && f.mime_type.starts_with("image/"))
    });
    if !has_media {
        return Value::String(c.text_concat());
    }
    let mut parts: Vec<Value> = Vec::new();
    for p in &c.parts {
        match p {
            Part::Text(t) if !t.is_empty() => {
                parts.push(serde_json::json!({ "type": "text", "text": t }));
            }
            Part::InlineData(d) if d.mime_type.starts_with("image/") => {
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": { "url": format!("data:{};base64,{}", d.mime_type, d.data) },
                }));
            }
            Part::FileData(f)
                if f.file_uri.starts_with("https://") && f.mime_type.starts_with("image/") =>
            {
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": { "url": f.file_uri },
                }));
            }
            Part::Text(_) | Part::Thought(_) | Part::RedactedThought(_) => {}
            other => tracing::warn!(
                part = ?std::mem::discriminant(other),
                "dropping part unsupported by the chat/completions API"
            ),
        }
    }
    Value::Array(parts)
}

#[derive(Debug, Serialize)]
pub(crate) struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireToolCallFunction,
}

#[derive(Debug, Serialize)]
pub(crate) struct WireToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireToolFunction,
}

#[derive(Debug, Serialize)]
pub(crate) struct WireToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub(crate) fn to_wire<'a>(req: &'a LlmRequest, model: &'a str) -> WireRequest<'a> {
    let mut messages: Vec<WireMessage> = Vec::new();
    if let Some(sys) = &req.config.system_instruction {
        let text = sys.text_concat();
        if !text.is_empty() {
            messages.push(WireMessage {
                role: "system",
                content: Some(Value::String(text)),
                name: None,
                tool_call_id: None,
                tool_calls: vec![],
            });
        }
    }
    for c in &req.contents {
        match c.role {
            Role::User | Role::System => messages.push(WireMessage {
                role: "user",
                content: Some(user_content(c)),
                name: None,
                tool_call_id: None,
                tool_calls: vec![],
            }),
            Role::Model => {
                let mut text = String::new();
                let mut tcs: Vec<WireToolCall> = Vec::new();
                for p in &c.parts {
                    match p {
                        Part::Text(t) => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        Part::FunctionCall(fc) => tcs.push(WireToolCall {
                            id: fc.id.clone().unwrap_or_else(random_id),
                            kind: "function",
                            function: WireToolCallFunction {
                                name: fc.name.clone(),
                                arguments: serde_json::to_string(&fc.args).unwrap_or_default(),
                            },
                        }),
                        _ => {}
                    }
                }
                messages.push(WireMessage {
                    role: "assistant",
                    content: if text.is_empty() {
                        None
                    } else {
                        Some(Value::String(text))
                    },
                    name: None,
                    tool_call_id: None,
                    tool_calls: tcs,
                });
            }
            Role::Tool => {
                for p in &c.parts {
                    if let Part::FunctionResponse(fr) = p {
                        let body = serde_json::to_string(&fr.response).unwrap_or_default();
                        messages.push(WireMessage {
                            role: "tool",
                            content: Some(Value::String(body)),
                            name: Some(fr.name.clone()),
                            tool_call_id: fr.id.clone(),
                            tool_calls: vec![],
                        });
                    }
                }
            }
        }
    }

    let tools: Vec<WireTool> = req
        .config
        .tools
        .iter()
        .filter_map(|t| match t {
            crate::genai_types::Tool::FunctionDeclarations(d) => Some(d),
            _ => None,
        })
        .flat_map(|d| d.iter().map(declaration_to_wire))
        .collect();

    let response_format = match (&req.config.response_mime_type, &req.config.response_schema) {
        // With a schema: strict structured outputs — the server enforces the
        // shape. Optional properties are transformed to required-but-nullable
        // (an OpenAI strict-mode constraint); see
        // `providers::common::to_json_schema`.
        (Some(m), Some(schema)) if m == "application/json" => Some(serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "response",
                "strict": true,
                "schema": crate::providers::common::to_json_schema(schema, true),
            },
        })),
        // JSON mime type without a schema: plain JSON mode.
        (Some(m), None) if m == "application/json" => Some(serde_json::json!({
            "type": "json_object",
        })),
        _ => None,
    };

    // Reasoning models (o-series, gpt-5 family) reject the deprecated
    // `max_tokens` with a 400 and require `max_completion_tokens`. Older
    // OpenAI-compatible servers (Ollama, older Azure api-versions) only know
    // `max_tokens`, so pick per model family rather than always sending the
    // new form.
    let bare_model = model.split('/').next_back().unwrap_or(model);
    let reasoning_family = ["o1", "o3", "o4", "gpt-5"]
        .iter()
        .any(|p| bare_model.starts_with(p));
    let (max_tokens, max_completion_tokens) = if reasoning_family {
        (None, req.config.max_output_tokens)
    } else {
        (req.config.max_output_tokens, None)
    };

    WireRequest {
        model,
        messages,
        tools,
        temperature: req.config.temperature,
        top_p: req.config.top_p,
        max_tokens,
        max_completion_tokens,
        stop: req.config.stop_sequences.clone(),
        stream: false,
        stream_options: None,
        response_format,
        seed: req.config.seed,
        presence_penalty: req.config.presence_penalty,
        frequency_penalty: req.config.frequency_penalty,
    }
}

fn declaration_to_wire(d: &FunctionDeclaration) -> WireTool {
    let params = d
        .parameters
        .as_ref()
        .map(|s| serde_json::to_value(s).unwrap_or_else(|_| serde_json::json!({"type": "object"})))
        .unwrap_or_else(|| serde_json::json!({"type": "object"}));
    let params = lowercase_type_recursive(params);
    WireTool {
        kind: "function",
        function: WireToolFunction {
            name: d.name.clone(),
            description: d.description.clone(),
            parameters: params,
        },
    }
}

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

fn random_id() -> String {
    format!("call_{}", uuid::Uuid::new_v4().simple())
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireResponse {
    pub model: Option<String>,
    pub choices: Vec<WireChoice>,
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireChoice {
    pub message: WireResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireResponseMessage {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<WireResponseToolCall>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireResponseToolCall {
    pub id: String,
    pub function: WireResponseFunction,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireResponseFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

pub(crate) fn parse_response(body: &[u8]) -> Result<LlmResponse> {
    let r: WireResponse = serde_json::from_slice(body)
        .map_err(|e| ProviderError::Decode(format!("openai response: {e}")))?;
    let mut parts: Vec<Part> = Vec::new();
    let finish_reason = if let Some(choice) = r.choices.into_iter().next() {
        if let Some(text) = choice.message.content {
            if !text.is_empty() {
                parts.push(Part::Text(text));
            }
        }
        for tc in choice.message.tool_calls {
            let args: Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            parts.push(Part::FunctionCall(FunctionCall {
                id: Some(tc.id),
                name: tc.function.name,
                args,
                thought_signature: None,
            }));
        }
        match choice.finish_reason.as_deref() {
            Some("stop") => Some(FinishReason::Stop),
            Some("length") => Some(FinishReason::MaxTokens),
            Some("tool_calls") => Some(FinishReason::Stop),
            Some("content_filter") => Some(FinishReason::Safety),
            _ => None,
        }
    } else {
        None
    };
    Ok(LlmResponse {
        model_version: r.model,
        content: Some(Content {
            role: Role::Model,
            parts,
        }),
        finish_reason,
        usage_metadata: r.usage.map(|u| UsageMetadata {
            prompt_token_count: Some(u.prompt_tokens),
            candidates_token_count: Some(u.completion_tokens),
            total_token_count: Some(u.prompt_tokens + u.completion_tokens),
            ..UsageMetadata::default()
        }),
        ..LlmResponse::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn to_wire_emits_system_and_user_messages() {
        let mut req = LlmRequest::default();
        req.config.system_instruction = Some(Content::system_text("be brief"));
        req.contents.push(Content::user_text("hi"));
        let w = to_wire(&req, "gpt-4o");
        assert_eq!(w.messages.len(), 2);
        assert_eq!(w.messages[0].role, "system");
        assert_eq!(w.messages[1].role, "user");
    }

    #[test]
    fn parse_response_text() {
        let body = json!({
            "model": "gpt-4o-mini",
            "choices": [{"message": {"content": "hi"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1}
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        assert_eq!(r.content.unwrap().text_concat(), "hi");
        assert_eq!(r.finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn reasoning_models_get_max_completion_tokens() {
        let mut req = LlmRequest::default();
        req.config.max_output_tokens = Some(1024);
        // Chat-tier model: legacy field.
        let w = serde_json::to_value(to_wire(&req, "gpt-4o-mini")).unwrap();
        assert_eq!(w["max_tokens"], 1024);
        assert!(w.get("max_completion_tokens").is_none());
        // Reasoning-tier models (with and without a routing prefix): new field.
        for m in ["o3-mini", "gpt-5", "openai/o1-preview"] {
            let w = serde_json::to_value(to_wire(&req, m)).unwrap();
            assert!(w.get("max_tokens").is_none(), "{m} sent max_tokens");
            assert_eq!(w["max_completion_tokens"], 1024, "{m}");
        }
    }

    #[test]
    fn text_only_user_content_stays_a_string() {
        let mut req = LlmRequest::default();
        req.contents.push(Content::user_text("hi"));
        let w = serde_json::to_value(to_wire(&req, "gpt-4o")).unwrap();
        assert_eq!(w["messages"][0]["content"], "hi");
    }

    #[test]
    fn inline_image_becomes_image_url_data_uri() {
        use crate::genai_types::part::InlineData;
        let mut req = LlmRequest::default();
        req.contents.push(Content {
            role: crate::genai_types::Role::User,
            parts: vec![
                Part::Text("what is this?".into()),
                Part::InlineData(InlineData::from_bytes("image/png", b"px")),
            ],
        });
        let w = serde_json::to_value(to_wire(&req, "gpt-4o")).unwrap();
        let parts = w["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
        assert!(
            parts[1]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }

    #[test]
    fn https_image_file_becomes_image_url() {
        use crate::genai_types::part::FileData;
        let mut req = LlmRequest::default();
        req.contents.push(Content {
            role: crate::genai_types::Role::User,
            parts: vec![Part::FileData(FileData {
                mime_type: "image/jpeg".into(),
                file_uri: "https://example.com/cat.jpg".into(),
                display_name: None,
            })],
        });
        let w = serde_json::to_value(to_wire(&req, "gpt-4o")).unwrap();
        assert_eq!(
            w["messages"][0]["content"][0]["image_url"]["url"],
            "https://example.com/cat.jpg"
        );
    }

    #[test]
    fn response_schema_maps_to_strict_json_schema() {
        use crate::genai_types::Schema;
        let mut req = LlmRequest::default();
        req.set_output_schema(
            Schema::object()
                .property("name", Schema::string())
                .property("age", Schema::integer())
                .require("name"),
        );
        let w = serde_json::to_value(to_wire(&req, "gpt-4o")).unwrap();
        let rf = &w["response_format"];
        assert_eq!(rf["type"], "json_schema");
        assert_eq!(rf["json_schema"]["name"], "response");
        assert_eq!(rf["json_schema"]["strict"], true);
        let schema = &rf["json_schema"]["schema"];
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"].as_array().unwrap().len(), 2);
        assert_eq!(
            schema["properties"]["age"]["type"],
            serde_json::json!(["integer", "null"])
        );
    }

    #[test]
    fn json_mime_without_schema_maps_to_json_object() {
        let mut req = LlmRequest::default();
        req.config.response_mime_type = Some("application/json".into());
        let w = serde_json::to_value(to_wire(&req, "gpt-4o")).unwrap();
        assert_eq!(w["response_format"]["type"], "json_object");
    }

    #[test]
    fn parse_response_tool_call() {
        let body = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "tc-1",
                        "type": "function",
                        "function": {"name": "f", "arguments": "{\"x\":1}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        let calls = r.function_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].args["x"], 1);
    }
}
