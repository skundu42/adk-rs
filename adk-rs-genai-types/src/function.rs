//! Function-call and function-response payloads (model ⇄ tool).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A model-emitted request to invoke a tool by name with JSON arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Stable id; some providers (Anthropic, OpenAI) require it for matching
    /// the corresponding `FunctionResponse`. Gemini may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Tool name.
    pub name: String,
    /// JSON object of arguments (per the tool's schema).
    #[serde(default)]
    pub args: Value,
}

impl FunctionCall {
    /// Construct a function call with no id.
    pub fn new(name: impl Into<String>, args: Value) -> Self {
        Self {
            id: None,
            name: name.into(),
            args,
        }
    }

    /// Set the id (used by Anthropic/OpenAI).
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// Whether and how a model should yield while waiting for a long-running
/// tool's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Scheduling {
    /// Yield immediately.
    Silent,
    /// Run synchronously.
    WhenIdle,
    /// Interrupt and report.
    Interrupt,
}

/// A tool-emitted response to a [`FunctionCall`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionResponse {
    /// Matches `FunctionCall::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Tool name.
    pub name: String,
    /// JSON object response.
    #[serde(default)]
    pub response: Value,
    /// Whether the tool will continue producing further responses.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "willContinue"
    )]
    pub will_continue: Option<bool>,
    /// Scheduling hint for long-running tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduling: Option<Scheduling>,
}

impl FunctionResponse {
    /// Construct a response.
    pub fn new(name: impl Into<String>, response: Value) -> Self {
        Self {
            id: None,
            name: name.into(),
            response,
            will_continue: None,
            scheduling: None,
        }
    }

    /// Set the matching call id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn function_call_round_trips() {
        let fc = FunctionCall::new("get_weather", json!({"city": "Paris"})).with_id("call-1");
        let j = serde_json::to_value(&fc).unwrap();
        assert_eq!(j["name"], "get_weather");
        assert_eq!(j["id"], "call-1");
        let back: FunctionCall = serde_json::from_value(j).unwrap();
        assert_eq!(back, fc);
    }

    #[test]
    fn function_response_omits_unset_optionals() {
        let fr = FunctionResponse::new("noop", json!({"ok": true}));
        let j = serde_json::to_string(&fr).unwrap();
        assert!(!j.contains("willContinue"));
        assert!(!j.contains("scheduling"));
        assert!(!j.contains(r#""id""#));
    }
}
