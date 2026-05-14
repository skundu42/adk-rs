//! Tool declarations (the data the model sees) and the `Tool` wrapper Gemini
//! uses in `GenerateContentConfig.tools`.

use serde::{Deserialize, Serialize};

use crate::genai_types::schema::Schema;

/// A declared, callable function the model can choose to invoke.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FunctionDeclaration {
    /// Tool name (matches `FunctionCall.name`).
    pub name: String,
    /// Human description shown to the model.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// JSON Schema describing the args object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Schema>,
    /// Schema for the function's response (optional, rarely used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Schema>,
}

impl FunctionDeclaration {
    /// Construct.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: None,
            response: None,
        }
    }

    /// Set the parameters schema.
    #[must_use]
    pub fn with_parameters(mut self, schema: Schema) -> Self {
        self.parameters = Some(schema);
        self
    }
}

/// A Gemini-style tool wrapper. Each variant maps to one entry in the wire
/// `tools: [...]` array. The variants for the server-side built-ins
/// (`GoogleSearch`, `UrlContext`, `CodeExecution`) carry an empty struct so
/// they serialise as `{"googleSearch": {}}` / `{"urlContext": {}}` /
/// `{"codeExecution": {}}` — Gemini's expected wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tool {
    /// A list of function declarations.
    FunctionDeclarations(Vec<FunctionDeclaration>),
    /// The built-in Google Search grounding tool (Gemini-only).
    GoogleSearch {},
    /// The built-in URL-context grounding tool (Gemini 2+).
    UrlContext {},
    /// The built-in server-side code-execution tool (Gemini-only).
    CodeExecution {},
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_types::schema::Schema;
    use serde_json::json;

    #[test]
    fn declaration_round_trips() {
        let d = FunctionDeclaration::new("noop", "do nothing").with_parameters(Schema::object());
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["name"], "noop");
        assert_eq!(j["parameters"]["type"], "OBJECT");
        let back: FunctionDeclaration = serde_json::from_value(j).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn builtin_tools_serialize_as_empty_objects() {
        assert_eq!(
            serde_json::to_value(Tool::GoogleSearch {}).unwrap(),
            json!({"googleSearch": {}})
        );
        assert_eq!(
            serde_json::to_value(Tool::UrlContext {}).unwrap(),
            json!({"urlContext": {}})
        );
        assert_eq!(
            serde_json::to_value(Tool::CodeExecution {}).unwrap(),
            json!({"codeExecution": {}})
        );
    }

    #[test]
    fn builtin_tools_round_trip() {
        for t in [
            Tool::GoogleSearch {},
            Tool::UrlContext {},
            Tool::CodeExecution {},
        ] {
            let j = serde_json::to_value(&t).unwrap();
            let back: Tool = serde_json::from_value(j).unwrap();
            assert_eq!(t, back);
        }
    }
}
