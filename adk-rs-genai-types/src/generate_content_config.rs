//! Provider-neutral generation parameters.

use serde::{Deserialize, Serialize};

use crate::content::Content;
use crate::schema::Schema;
use crate::tool::Tool;

/// Tool choice mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ToolMode {
    /// The model picks; tools are advisory.
    Auto,
    /// The model must call any tool.
    Any,
    /// The model must not call tools.
    None,
}

/// Tool selection configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolConfig {
    /// The function-calling mode.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "mode")]
    pub mode: Option<ToolMode>,
    /// Restrict the model to a subset of declared tool names.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "allowedFunctionNames"
    )]
    pub allowed_function_names: Option<Vec<String>>,
}

/// Gemini-specific harm category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmCategory {
    /// Unspecified.
    HarmCategoryUnspecified,
    /// Harassment.
    HarmCategoryHarassment,
    /// Hate speech.
    HarmCategoryHateSpeech,
    /// Sexually explicit content.
    HarmCategorySexuallyExplicit,
    /// Dangerous content.
    HarmCategoryDangerousContent,
    /// Civic integrity.
    HarmCategoryCivicIntegrity,
}

/// Threshold for blocking unsafe content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmBlockThreshold {
    /// Block none.
    BlockNone,
    /// Block only high severity.
    BlockOnlyHigh,
    /// Block medium and above.
    BlockMediumAndAbove,
    /// Block low and above.
    BlockLowAndAbove,
}

/// One safety setting (category → threshold).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetySetting {
    /// The harm category.
    pub category: HarmCategory,
    /// The block threshold.
    pub threshold: HarmBlockThreshold,
}

/// Thinking/reasoning budget configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Token budget for thinking. `0` disables; `None` means model default.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "thinkingBudget"
    )]
    pub thinking_budget: Option<i32>,
    /// Whether to include thoughts in the output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "includeThoughts"
    )]
    pub include_thoughts: Option<bool>,
}

/// Provider-neutral parameters for a `generateContent` call.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GenerateContentConfig {
    /// System instruction (Gemini's `systemInstruction` field).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "systemInstruction"
    )]
    pub system_instruction: Option<Content>,
    /// Available tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    /// Tool selection mode.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "toolConfig"
    )]
    pub tool_config: Option<ToolConfig>,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "topP")]
    pub top_p: Option<f32>,
    /// Top-k sampling.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "topK")]
    pub top_k: Option<u32>,
    /// Maximum tokens in the response.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "maxOutputTokens"
    )]
    pub max_output_tokens: Option<u32>,
    /// Candidate count.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "candidateCount"
    )]
    pub candidate_count: Option<u32>,
    /// Stop sequences.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "stopSequences"
    )]
    pub stop_sequences: Vec<String>,
    /// Response MIME type (e.g. `application/json`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "responseMimeType"
    )]
    pub response_mime_type: Option<String>,
    /// Structured response schema.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "responseSchema"
    )]
    pub response_schema: Option<Schema>,
    /// Safety filters.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "safetySettings"
    )]
    pub safety_settings: Vec<SafetySetting>,
    /// Thinking config.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "thinkingConfig"
    )]
    pub thinking_config: Option<ThinkingConfig>,
    /// Seed for determinism.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Presence penalty (OpenAI/Anthropic-compatible).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "presencePenalty"
    )]
    pub presence_penalty: Option<f32>,
    /// Frequency penalty.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "frequencyPenalty"
    )]
    pub frequency_penalty: Option<f32>,
}

impl GenerateContentConfig {
    /// Append the given text to (or set) the system instruction.
    pub fn append_system_text(&mut self, text: &str) {
        match self.system_instruction.as_mut() {
            Some(c) => {
                let combined = if c.text_concat().is_empty() {
                    text.to_string()
                } else {
                    format!("{}\n\n{}", c.text_concat(), text)
                };
                *c = Content::system_text(combined);
            }
            None => self.system_instruction = Some(Content::system_text(text)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_serialise_to_empty_object() {
        let c = GenerateContentConfig::default();
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j, serde_json::json!({}));
    }

    #[test]
    fn append_system_text_appends() {
        let mut c = GenerateContentConfig::default();
        c.append_system_text("hello");
        c.append_system_text("world");
        assert_eq!(
            c.system_instruction.as_ref().unwrap().text_concat(),
            "hello\n\nworld"
        );
    }
}
