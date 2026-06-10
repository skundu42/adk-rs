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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
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
}

#[derive(Debug, Serialize)]
pub(crate) struct WireTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
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
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WireToolResultPart {
    Text { text: String },
}

pub(crate) fn to_wire<'a>(req: &'a LlmRequest, model: &'a str) -> WireRequest<'a> {
    let system = req
        .config
        .system_instruction
        .as_ref()
        .map(crate::genai_types::Content::text_concat);

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

    let tools: Vec<WireTool> = req
        .config
        .tools
        .iter()
        .filter_map(|t| match t {
            crate::genai_types::Tool::FunctionDeclarations(decls) => Some(decls),
            _ => None,
        })
        .flat_map(|decls| decls.iter().map(declaration_to_wire))
        .collect();

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

    WireRequest {
        model,
        max_tokens: req.config.max_output_tokens.unwrap_or(2048),
        system,
        messages,
        tools,
        temperature: req.config.temperature,
        top_p: req.config.top_p,
        top_k: req.config.top_k,
        stop_sequences: req.config.stop_sequences.clone(),
        stream: false,
        output_config,
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
            Part::Text(t) | Part::Thought(t) if !t.is_empty() => {
                out.push(WireBlock::Text { text: t.clone() });
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
            // InlineData / FileData / ExecutableCode / CodeExecutionResult are
            // not supported in v0.1; silently skip.
            _ => {}
        }
    }
    (role, out)
}

fn random_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Anthropic response shape.
#[derive(Debug, Deserialize)]
pub(crate) struct WireResponse {
    pub content: Vec<WireResponseBlock>,
    pub stop_reason: Option<String>,
    pub model: Option<String>,
    pub usage: Option<WireUsage>,
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
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

pub(crate) fn parse_response(body: &[u8]) -> Result<LlmResponse> {
    let r: WireResponse = serde_json::from_slice(body)
        .map_err(|e| ProviderError::Decode(format!("anthropic response: {e}")))?;
    Ok(from_wire_response(r))
}

pub(crate) fn from_wire_response(r: WireResponse) -> LlmResponse {
    let mut parts: Vec<Part> = Vec::with_capacity(r.content.len());
    for b in r.content {
        match b {
            WireResponseBlock::Text { text } => parts.push(Part::Text(text)),
            WireResponseBlock::Thinking { thinking } => parts.push(Part::Thought(thinking)),
            WireResponseBlock::ToolUse { id, name, input } => {
                parts.push(Part::FunctionCall(FunctionCall {
                    id: Some(id),
                    name,
                    args: input,
                }))
            }
        }
    }
    let finish = match r.stop_reason.as_deref() {
        Some("end_turn") | Some("stop_sequence") => Some(FinishReason::Stop),
        Some("max_tokens") => Some(FinishReason::MaxTokens),
        Some("tool_use") => Some(FinishReason::Stop),
        _ => None,
    };
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
            ..UsageMetadata::default()
        }),
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
