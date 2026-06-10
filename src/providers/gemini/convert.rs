//! Conversion between adk-rs types and Gemini's wire format.
//!
//! Most of the work is mechanical: our [`adk_rs_genai_types`] mirror Gemini's
//! Pydantic shapes. The places we deviate (system instruction handling,
//! schema sanitization) are documented inline.

use serde::Serialize;

use crate::core::{LlmRequest, LlmResponse};
use crate::error::Result;
use crate::genai_types::{Content, GenerateContentConfig, GenerateContentResponse};

/// Gemini wire-format request body.
#[derive(Debug, Serialize)]
pub(crate) struct WireRequest<'a> {
    pub contents: &'a [Content],
    /// `systemInstruction` is `Content`-shaped on Gemini.
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemInstruction")]
    pub system_instruction: Option<&'a Content>,
    #[serde(skip_serializing_if = "is_empty_slice")]
    pub tools: &'a [crate::genai_types::Tool],
    #[serde(skip_serializing_if = "Option::is_none", rename = "toolConfig")]
    pub tool_config: Option<&'a crate::genai_types::ToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "generationConfig")]
    pub generation_config: Option<GenerationConfig<'a>>,
    #[serde(skip_serializing_if = "is_empty_slice", rename = "safetySettings")]
    pub safety_settings: &'a [crate::genai_types::SafetySetting],
    /// Reference to an explicit server-side cache entry. When set, the
    /// cached fields (system instruction + tools) must be omitted.
    #[serde(skip_serializing_if = "Option::is_none", rename = "cachedContent")]
    pub cached_content: Option<&'a str>,
}

/// Body for `POST /{version}/cachedContents`.
#[derive(Debug, Serialize)]
pub(crate) struct WireCachedContentCreate<'a> {
    /// Fully-qualified model name, e.g. `models/gemini-2.5-flash`.
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemInstruction")]
    pub system_instruction: Option<&'a Content>,
    #[serde(skip_serializing_if = "is_empty_slice")]
    pub tools: &'a [crate::genai_types::Tool],
    /// TTL like `"1800s"`.
    pub ttl: String,
}

fn is_empty_slice<T>(s: &&[T]) -> bool {
    s.is_empty()
}

/// `GenerateContentConfig` minus the fields Gemini puts elsewhere.
#[derive(Debug, Serialize)]
pub(crate) struct GenerationConfig<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "topP")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "topK")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxOutputTokens")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "candidateCount")]
    pub candidate_count: Option<u32>,
    #[serde(skip_serializing_if = "is_empty_slice", rename = "stopSequences")]
    pub stop_sequences: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none", rename = "responseMimeType")]
    pub response_mime_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "responseSchema")]
    pub response_schema: Option<&'a crate::genai_types::Schema>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinkingConfig")]
    pub thinking_config: Option<&'a crate::genai_types::ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "presencePenalty")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "frequencyPenalty")]
    pub frequency_penalty: Option<f32>,
}

fn split_config(c: &GenerateContentConfig) -> Option<GenerationConfig<'_>> {
    let any = c.temperature.is_some()
        || c.top_p.is_some()
        || c.top_k.is_some()
        || c.max_output_tokens.is_some()
        || c.candidate_count.is_some()
        || !c.stop_sequences.is_empty()
        || c.response_mime_type.is_some()
        || c.response_schema.is_some()
        || c.thinking_config.is_some()
        || c.seed.is_some()
        || c.presence_penalty.is_some()
        || c.frequency_penalty.is_some();
    if !any {
        return None;
    }
    Some(GenerationConfig {
        temperature: c.temperature,
        top_p: c.top_p,
        top_k: c.top_k,
        max_output_tokens: c.max_output_tokens,
        candidate_count: c.candidate_count,
        stop_sequences: &c.stop_sequences,
        response_mime_type: c.response_mime_type.as_deref(),
        response_schema: c.response_schema.as_ref(),
        thinking_config: c.thinking_config.as_ref(),
        seed: c.seed,
        presence_penalty: c.presence_penalty,
        frequency_penalty: c.frequency_penalty,
    })
}

pub(crate) fn to_wire(req: &LlmRequest) -> WireRequest<'_> {
    WireRequest {
        contents: &req.contents,
        system_instruction: req.config.system_instruction.as_ref(),
        tools: &req.config.tools,
        tool_config: req.config.tool_config.as_ref(),
        generation_config: split_config(&req.config),
        safety_settings: &req.config.safety_settings,
        cached_content: None,
    }
}

/// Like [`to_wire`] but referencing an explicit cache entry: the cached
/// prefix (system instruction + tools) is omitted from the body.
pub(crate) fn to_wire_cached<'a>(req: &'a LlmRequest, cache_name: &'a str) -> WireRequest<'a> {
    WireRequest {
        contents: &req.contents,
        system_instruction: None,
        tools: &[],
        tool_config: req.config.tool_config.as_ref(),
        generation_config: split_config(&req.config),
        safety_settings: &req.config.safety_settings,
        cached_content: Some(cache_name),
    }
}

/// Decode a [`GenerateContentResponse`] from a JSON body.
pub(crate) fn parse_response(body: &[u8]) -> Result<LlmResponse> {
    let resp: GenerateContentResponse = serde_json::from_slice(body)?;
    Ok(LlmResponse::from_generate(resp))
}

/// Decode a single SSE chunk into an [`LlmResponse`].
pub(crate) fn parse_stream_chunk(payload: &str) -> Result<LlmResponse> {
    let resp: GenerateContentResponse = serde_json::from_str(payload)?;
    let mut r = LlmResponse::from_generate(resp);
    // Streamed chunks are partial unless they carry a `STOP` finish reason.
    if r.finish_reason
        .map(|f| matches!(f, crate::genai_types::FinishReason::Stop))
        .unwrap_or(false)
    {
        // final
    } else if r.content.is_some() {
        r = LlmResponse {
            // mark as a partial in-progress chunk
            finish_reason: r.finish_reason,
            ..r
        };
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::LlmRequest;
    use crate::genai_types::{
        Content, FunctionCall, GenerateContentConfig, Part, Role, Schema, Tool,
    };
    use serde_json::json;

    #[test]
    fn to_wire_includes_only_set_fields() {
        let mut req = LlmRequest::default();
        req.contents.push(Content::user_text("hi"));
        let body = serde_json::to_value(to_wire(&req)).unwrap();
        assert_eq!(body["contents"][0]["role"], "user");
        assert!(body.get("systemInstruction").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("generationConfig").is_none());
    }

    #[test]
    fn to_wire_emits_function_decls_in_tools() {
        let mut req = LlmRequest::default();
        req.config.tools.push(Tool::FunctionDeclarations(vec![
            crate::genai_types::FunctionDeclaration::new("f", "do f")
                .with_parameters(Schema::object()),
        ]));
        let body = serde_json::to_value(to_wire(&req)).unwrap();
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "f");
    }

    #[test]
    fn to_wire_serializes_gemini_builtin_tools() {
        let mut req = LlmRequest::default();
        req.config.tools.push(Tool::GoogleSearch {});
        req.config.tools.push(Tool::UrlContext {});
        req.config.tools.push(Tool::CodeExecution {});
        let body = serde_json::to_value(to_wire(&req)).unwrap();
        assert_eq!(body["tools"][0], json!({"googleSearch": {}}));
        assert_eq!(body["tools"][1], json!({"urlContext": {}}));
        assert_eq!(body["tools"][2], json!({"codeExecution": {}}));
    }

    #[test]
    fn parse_response_unwraps_first_candidate() {
        let body = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "hello"}]},
                "finishReason": "STOP"
            }],
            "modelVersion": "gemini-2.5-flash"
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        assert_eq!(r.content.as_ref().unwrap().text_concat(), "hello");
        assert_eq!(r.model_version.as_deref(), Some("gemini-2.5-flash"));
    }

    #[test]
    fn parse_function_call_response() {
        let body = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"functionCall": {"name": "get_weather", "args": {"city": "Paris"}}}
                ]},
                "finishReason": "STOP"
            }]
        });
        let r = parse_response(body.to_string().as_bytes()).unwrap();
        let calls: Vec<FunctionCall> = r.function_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
    }

    #[test]
    fn build_request_with_system_instruction() {
        let mut req = LlmRequest::default();
        req.config = GenerateContentConfig {
            system_instruction: Some(Content::system_text("be brief")),
            ..GenerateContentConfig::default()
        };
        req.contents.push(Content {
            role: Role::User,
            parts: vec![Part::text("hi")],
        });
        let body = serde_json::to_value(to_wire(&req)).unwrap();
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be brief");
    }
}
