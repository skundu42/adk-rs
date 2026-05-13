//! [`LlmRequest`] — a provider-neutral request bundle.
//!
//! Wraps the wire-level `GenerateContentConfig` plus an ADK-internal
//! `tools_dict` so the runner can dispatch named tool calls back to live
//! [`Tool`](crate::model::Model) implementations.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use adk_rs_genai_types::{Content, FunctionDeclaration, GenerateContentConfig, Tool, schema::Schema};

/// A unified request object sent to a [`crate::model::Model`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Model identifier (e.g. `gemini-2.5-flash`, `claude-3-5-sonnet`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Conversation contents (no system instruction here — see `config`).
    #[serde(default)]
    pub contents: Vec<Content>,
    /// Generation config (system instruction, tools, sampling, ...).
    #[serde(default)]
    pub config: GenerateContentConfig,
    /// Map of `tool_name -> Tool` used by the runner to dispatch calls
    /// emitted by the model. Skipped in serialization (`Arc<dyn ...>` is
    /// not serializable; we round-trip the declarations via `config.tools`).
    #[serde(skip)]
    pub tools_dict: HashMap<String, Arc<dyn crate::tool_object::DynTool>>,
}

// Re-export removed — see `crate::tool_object::DynTool` directly.

impl LlmRequest {
    /// Append system instructions to the system prompt.
    pub fn append_system_text(&mut self, text: &str) {
        self.config.append_system_text(text);
    }

    /// Append tool declarations to `config.tools`, merging into an existing
    /// `FunctionDeclarations` entry if one is present.
    pub fn append_function_declarations(
        &mut self,
        decls: impl IntoIterator<Item = FunctionDeclaration>,
    ) {
        let mut new: Vec<FunctionDeclaration> = decls.into_iter().collect();
        if new.is_empty() {
            return;
        }
        for tool in &mut self.config.tools {
            if let Tool::FunctionDeclarations(d) = tool {
                d.append(&mut new);
                return;
            }
        }
        self.config.tools.push(Tool::FunctionDeclarations(new));
    }

    /// Set the structured response schema and force JSON output.
    pub fn set_output_schema(&mut self, schema: Schema) {
        self.config.response_schema = Some(schema);
        self.config.response_mime_type = Some("application/json".into());
    }
}

impl PartialEq for LlmRequest {
    fn eq(&self, other: &Self) -> bool {
        // Compare only serialisable fields. tools_dict is a runtime registry.
        self.model == other.model && self.contents == other.contents && self.config == other.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_rs_genai_types::schema::Schema;

    #[test]
    fn append_function_declarations_merges_lists() {
        let mut r = LlmRequest::default();
        r.append_function_declarations([FunctionDeclaration::new("a", "")]);
        r.append_function_declarations([FunctionDeclaration::new("b", "")]);
        // One Tool::FunctionDeclarations with both inside.
        assert_eq!(r.config.tools.len(), 1);
        let Tool::FunctionDeclarations(d) = &r.config.tools[0] else {
            unreachable!();
        };
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn set_output_schema_forces_json_mime() {
        let mut r = LlmRequest::default();
        r.set_output_schema(Schema::object());
        assert_eq!(
            r.config.response_mime_type.as_deref(),
            Some("application/json")
        );
        assert!(r.config.response_schema.is_some());
    }
}
