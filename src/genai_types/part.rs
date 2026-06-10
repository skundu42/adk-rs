//! The unit of content carried in [`Content`](crate::genai_types::Content).
//!
//! Gemini's wire format discriminates [`Part`] variants by *field presence*,
//! not by a `kind` tag. We therefore hand-write `Serialize`/`Deserialize`
//! so error messages are useful and round-trips with Python ADK stay byte-
//! compatible.

use serde::de::{self, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::genai_types::function::{FunctionCall, FunctionResponse};

/// Inline binary data embedded directly in the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineData {
    /// MIME type (e.g. `image/png`).
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    /// Base64-encoded bytes (we keep it as the encoded `String` to match the
    /// wire format directly; helpers below decode/encode raw bytes).
    pub data: String,
    /// Optional display name.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "displayName"
    )]
    pub display_name: Option<String>,
}

impl InlineData {
    /// Construct from raw bytes; the bytes are base64-encoded.
    pub fn from_bytes(mime_type: impl Into<String>, bytes: &[u8]) -> Self {
        use base64::Engine as _;
        Self {
            mime_type: mime_type.into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            display_name: None,
        }
    }

    /// Decode the base64 payload back into bytes.
    pub fn decode(&self) -> Result<Vec<u8>, base64::DecodeError> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(&self.data)
    }
}

/// External file reference (e.g. a `gs://`, `https://` URI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileData {
    /// MIME type.
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    /// URI of the file.
    #[serde(rename = "fileUri")]
    pub file_uri: String,
    /// Optional display name.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "displayName"
    )]
    pub display_name: Option<String>,
}

/// Code emitted by the model for execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableCode {
    /// Programming language (e.g. `PYTHON`).
    pub language: String,
    /// Source code.
    pub code: String,
}

/// Outcome of executing a [`ExecutableCode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    /// Unspecified outcome.
    OutcomeUnspecified,
    /// Execution succeeded.
    OutcomeOk,
    /// Execution failed.
    OutcomeFailed,
    /// Execution timed out / deadline exceeded.
    OutcomeDeadlineExceeded,
}

/// The result of executing an [`ExecutableCode`] part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeExecutionResult {
    /// Outcome enum.
    pub outcome: Outcome,
    /// Combined stdout/stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// A reasoning ("thought") trace plus the provider-issued signature that
/// must be echoed back verbatim on later turns (Anthropic thinking-block
/// `signature`, Gemini `thoughtSignature`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Thought {
    /// The visible reasoning text.
    pub text: String,
    /// Provider signature. Round-trips opaquely; never synthesise one.
    pub signature: Option<String>,
}

impl Thought {
    /// Construct an unsigned thought.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            signature: None,
        }
    }

    /// Attach the provider signature.
    #[must_use]
    pub fn with_signature(mut self, sig: impl Into<String>) -> Self {
        self.signature = Some(sig.into());
        self
    }
}

/// A discriminated content unit. See module docs for the rationale behind the
/// hand-written `Serialize`/`Deserialize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Plain text.
    Text(String),
    /// Inline binary blob.
    InlineData(InlineData),
    /// External file reference.
    FileData(FileData),
    /// Model-emitted function call.
    FunctionCall(FunctionCall),
    /// Tool-emitted function response.
    FunctionResponse(FunctionResponse),
    /// Code to be executed.
    ExecutableCode(ExecutableCode),
    /// Result of code execution.
    CodeExecutionResult(CodeExecutionResult),
    /// "Thought" trace (model reasoning), with optional provider signature.
    Thought(Thought),
    /// Encrypted reasoning the provider withheld (Anthropic
    /// `redacted_thinking`). The opaque payload must be echoed back
    /// verbatim on later turns; it is never human-readable.
    RedactedThought(String),
}

impl Part {
    /// Convenience: build a text part.
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    /// Convenience: build an unsigned thought part.
    pub fn thought(s: impl Into<String>) -> Self {
        Self::Thought(Thought::new(s))
    }

    /// Convenience: build an inline-data part from raw bytes.
    pub fn inline_bytes(mime_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self::InlineData(InlineData::from_bytes(mime_type, bytes))
    }

    /// Get the contained text, if any.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(t) => Some(t),
            Self::Thought(t) => Some(&t.text),
            _ => None,
        }
    }

    /// Get the contained function call, if any.
    #[must_use]
    pub fn as_function_call(&self) -> Option<&FunctionCall> {
        if let Self::FunctionCall(fc) = self {
            Some(fc)
        } else {
            None
        }
    }

    /// Get the contained function response, if any.
    #[must_use]
    pub fn as_function_response(&self) -> Option<&FunctionResponse> {
        if let Self::FunctionResponse(fr) = self {
            Some(fr)
        } else {
            None
        }
    }
}

impl Serialize for Part {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        match self {
            Self::Text(t) => m.serialize_entry("text", t)?,
            Self::Thought(t) => {
                // Gemini represents a thought as `{text: "...", thought: true}`
                // with the signature as a sibling `thoughtSignature` key.
                m.serialize_entry("text", &t.text)?;
                m.serialize_entry("thought", &true)?;
                if let Some(sig) = &t.signature {
                    m.serialize_entry("thoughtSignature", sig)?;
                }
            }
            Self::RedactedThought(data) => {
                // adk extension (Anthropic `redacted_thinking`); Gemini has
                // no equivalent and the provider converters handle it
                // before serialization.
                m.serialize_entry("redactedThought", data)?;
            }
            Self::InlineData(d) => m.serialize_entry("inlineData", d)?,
            Self::FileData(d) => m.serialize_entry("fileData", d)?,
            Self::FunctionCall(c) => {
                // The thought signature rides at the part level (Gemini's
                // wire shape), not inside the functionCall object.
                if let Some(sig) = &c.thought_signature {
                    let mut stripped = c.clone();
                    stripped.thought_signature = None;
                    m.serialize_entry("functionCall", &stripped)?;
                    m.serialize_entry("thoughtSignature", sig)?;
                } else {
                    m.serialize_entry("functionCall", c)?;
                }
            }
            Self::FunctionResponse(r) => m.serialize_entry("functionResponse", r)?,
            Self::ExecutableCode(c) => m.serialize_entry("executableCode", c)?,
            Self::CodeExecutionResult(r) => m.serialize_entry("codeExecutionResult", r)?,
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for Part {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Part;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a Part object with exactly one content field")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Part, A::Error> {
                let mut text: Option<String> = None;
                let mut thought: Option<bool> = None;
                let mut thought_signature: Option<String> = None;
                let mut redacted_thought: Option<String> = None;
                let mut inline_data: Option<InlineData> = None;
                let mut file_data: Option<FileData> = None;
                let mut function_call: Option<FunctionCall> = None;
                let mut function_response: Option<FunctionResponse> = None;
                let mut executable_code: Option<ExecutableCode> = None;
                let mut code_execution_result: Option<CodeExecutionResult> = None;
                let mut unknown: Option<String> = None;
                while let Some(key) = m.next_key::<String>()? {
                    match key.as_str() {
                        "text" => text = Some(m.next_value()?),
                        "thought" => thought = Some(m.next_value()?),
                        "thoughtSignature" | "thought_signature" => {
                            thought_signature = Some(m.next_value()?);
                        }
                        "redactedThought" | "redacted_thought" => {
                            redacted_thought = Some(m.next_value()?);
                        }
                        "inlineData" | "inline_data" => inline_data = Some(m.next_value()?),
                        "fileData" | "file_data" => file_data = Some(m.next_value()?),
                        "functionCall" | "function_call" => function_call = Some(m.next_value()?),
                        "functionResponse" | "function_response" => {
                            function_response = Some(m.next_value()?);
                        }
                        "executableCode" | "executable_code" => {
                            executable_code = Some(m.next_value()?);
                        }
                        "codeExecutionResult" | "code_execution_result" => {
                            code_execution_result = Some(m.next_value()?);
                        }
                        // Tolerate but record unknown keys so the error
                        // message points at the right thing.
                        other => {
                            // Swallow the value to keep the deserializer happy.
                            let _: Value = m.next_value()?;
                            unknown.get_or_insert_with(|| other.to_string());
                        }
                    }
                }

                if let Some(t) = text {
                    return Ok(if thought.unwrap_or(false) {
                        Part::Thought(Thought {
                            text: t,
                            signature: thought_signature,
                        })
                    } else {
                        Part::Text(t)
                    });
                }
                if let Some(data) = redacted_thought {
                    return Ok(Part::RedactedThought(data));
                }
                if let Some(d) = inline_data {
                    return Ok(Part::InlineData(d));
                }
                if let Some(d) = file_data {
                    return Ok(Part::FileData(d));
                }
                if let Some(mut c) = function_call {
                    // The signature rides at the part level on the wire.
                    if c.thought_signature.is_none() {
                        c.thought_signature = thought_signature;
                    }
                    return Ok(Part::FunctionCall(c));
                }
                if let Some(r) = function_response {
                    return Ok(Part::FunctionResponse(r));
                }
                if let Some(c) = executable_code {
                    return Ok(Part::ExecutableCode(c));
                }
                if let Some(r) = code_execution_result {
                    return Ok(Part::CodeExecutionResult(r));
                }
                match unknown {
                    Some(k) => Err(de::Error::custom(format!(
                        "Part has no recognised content field (found only `{k}`)"
                    ))),
                    None => Err(de::Error::custom("Part is empty")),
                }
            }
        }
        d.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_round_trip() {
        let p = Part::text("hello");
        let j = serde_json::to_value(&p).unwrap();
        assert_eq!(j, json!({"text": "hello"}));
        let back: Part = serde_json::from_value(j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn function_call_round_trip() {
        let p = Part::FunctionCall(FunctionCall::new("f", json!({"a": 1})).with_id("c1"));
        let j = serde_json::to_value(&p).unwrap();
        assert_eq!(j["functionCall"]["name"], "f");
        assert_eq!(j["functionCall"]["id"], "c1");
        let back: Part = serde_json::from_value(j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn inline_bytes_round_trip() {
        let p = Part::inline_bytes("text/plain", b"hi");
        let j = serde_json::to_value(&p).unwrap();
        assert_eq!(j["inlineData"]["mimeType"], "text/plain");
        let back: Part = serde_json::from_value(j).unwrap();
        assert_eq!(p, back);
        if let Part::InlineData(d) = back {
            assert_eq!(d.decode().unwrap(), b"hi");
        } else {
            unreachable!();
        }
    }

    #[test]
    fn thought_round_trip() {
        let p = Part::thought("thinking");
        let j = serde_json::to_value(&p).unwrap();
        assert_eq!(j, json!({"text": "thinking", "thought": true}));
        let back: Part = serde_json::from_value(j).unwrap();
        assert_eq!(p, back);
    }

    /// Thought signatures (Anthropic `signature`, Gemini `thoughtSignature`)
    /// must round-trip verbatim — providers reject replayed thinking that
    /// lost its signature.
    #[test]
    fn signed_thought_round_trip() {
        let p = Part::Thought(Thought::new("deep").with_signature("sig-abc"));
        let j = serde_json::to_value(&p).unwrap();
        assert_eq!(
            j,
            json!({"text": "deep", "thought": true, "thoughtSignature": "sig-abc"})
        );
        let back: Part = serde_json::from_value(j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn redacted_thought_round_trips() {
        let p = Part::RedactedThought("opaque-blob".into());
        let j = serde_json::to_value(&p).unwrap();
        assert_eq!(j, json!({"redactedThought": "opaque-blob"}));
        let back: Part = serde_json::from_value(j).unwrap();
        assert_eq!(p, back);
    }

    /// Gemini attaches `thoughtSignature` as a sibling of `functionCall`
    /// (mandatory to echo back on Gemini 3 function calling); it must
    /// survive the round-trip on the part, not nested in the call object.
    #[test]
    fn function_call_thought_signature_round_trips() {
        let mut fc = FunctionCall::new("f", json!({"x": 1}));
        fc.thought_signature = Some("sig-fc".into());
        let p = Part::FunctionCall(fc);
        let j = serde_json::to_value(&p).unwrap();
        assert_eq!(j["thoughtSignature"], "sig-fc");
        assert!(j["functionCall"].get("thoughtSignature").is_none());
        let back: Part = serde_json::from_value(j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn snake_case_keys_are_accepted() {
        let v = json!({"function_call": {"name": "f", "args": {"x": 1}}});
        let back: Part = serde_json::from_value(v).unwrap();
        match back {
            Part::FunctionCall(fc) => assert_eq!(fc.name, "f"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn unknown_key_reports_helpfully() {
        let v = json!({"bogus": 1});
        let err = serde_json::from_value::<Part>(v).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bogus"), "got: {msg}");
    }

    #[test]
    fn empty_part_errors_clearly() {
        let v = json!({});
        let err = serde_json::from_value::<Part>(v).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }
}
