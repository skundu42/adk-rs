//! Response shapes returned by `generateContent`.

use serde::{Deserialize, Serialize};

use crate::genai_types::content::Content;
use crate::genai_types::generate_content_config::HarmCategory;

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FinishReason {
    /// Unspecified.
    FinishReasonUnspecified,
    /// Normal completion.
    Stop,
    /// Truncated at `maxOutputTokens`.
    MaxTokens,
    /// Blocked by safety filters.
    Safety,
    /// Blocked due to recitation.
    Recitation,
    /// Blocked due to language not supported.
    Language,
    /// Provider-side error.
    Other,
    /// Blocked due to blocklist.
    Blocklist,
    /// Blocked for prohibited content.
    ProhibitedContent,
    /// SPII (sensitive PII).
    Spii,
    /// Malformed function call.
    MalformedFunctionCall,
}

/// One safety rating produced by the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyRating {
    /// Harm category.
    pub category: HarmCategory,
    /// Probability score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probability: Option<HarmProbability>,
    /// Whether the rating triggered blocking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
}

/// Probability rating bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmProbability {
    /// Unspecified.
    HarmProbabilityUnspecified,
    /// Negligible.
    Negligible,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
}

/// Grounding metadata (Google Search citations).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundingMetadata {
    /// Sources cited.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "groundingChunks"
    )]
    pub grounding_chunks: Vec<serde_json::Value>,
    /// Search entry point.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "searchEntryPoint"
    )]
    pub search_entry_point: Option<serde_json::Value>,
}

/// Citation metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationMetadata {
    /// List of citations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citations: Vec<serde_json::Value>,
}

/// A single candidate response from the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// The actual content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
    /// Why generation stopped.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "finishReason"
    )]
    pub finish_reason: Option<FinishReason>,
    /// Optional message explaining finish reason.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "finishMessage"
    )]
    pub finish_message: Option<String>,
    /// Safety ratings for this candidate.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "safetyRatings"
    )]
    pub safety_ratings: Vec<SafetyRating>,
    /// Grounding metadata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "groundingMetadata"
    )]
    pub grounding_metadata: Option<GroundingMetadata>,
    /// Citation metadata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "citationMetadata"
    )]
    pub citation_metadata: Option<CitationMetadata>,
    /// Average log-prob of generated tokens.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "avgLogprobs"
    )]
    pub avg_logprobs: Option<f64>,
}

/// Token usage info.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMetadata {
    /// Tokens in the prompt.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "promptTokenCount"
    )]
    pub prompt_token_count: Option<u32>,
    /// Tokens generated.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "candidatesTokenCount"
    )]
    pub candidates_token_count: Option<u32>,
    /// Total tokens.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "totalTokenCount"
    )]
    pub total_token_count: Option<u32>,
    /// Tokens served from a prompt cache.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "cachedContentTokenCount"
    )]
    pub cached_content_token_count: Option<u32>,
    /// Thinking-budget tokens used.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "thoughtsTokenCount"
    )]
    pub thoughts_token_count: Option<u32>,
}

/// Top-level prompt-feedback section (block reasons, etc.).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptFeedback {
    /// Block reason, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "blockReason"
    )]
    pub block_reason: Option<String>,
    /// Block reason message.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "blockReasonMessage"
    )]
    pub block_reason_message: Option<String>,
    /// Safety ratings.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "safetyRatings"
    )]
    pub safety_ratings: Vec<SafetyRating>,
}

/// A complete `generateContent` response (or one streaming chunk).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GenerateContentResponse {
    /// Response candidates (usually 1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
    /// Prompt feedback.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "promptFeedback"
    )]
    pub prompt_feedback: Option<PromptFeedback>,
    /// Usage metadata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "usageMetadata"
    )]
    pub usage_metadata: Option<UsageMetadata>,
    /// Model version string.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "modelVersion"
    )]
    pub model_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_response_round_trips() {
        let r = GenerateContentResponse::default();
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j, serde_json::json!({}));
        let back: GenerateContentResponse = serde_json::from_value(j).unwrap();
        assert_eq!(r, back);
    }
}
