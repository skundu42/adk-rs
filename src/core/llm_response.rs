//! [`LlmResponse`] — provider-neutral response payload, used by both single-
//! shot and streaming flows.

use serde::{Deserialize, Serialize};

use crate::genai_types::{
    Candidate, CitationMetadata, Content, FinishReason, FunctionCall, FunctionResponse,
    GenerateContentResponse, GroundingMetadata, UsageMetadata,
};

/// Provider-neutral response payload.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    /// Model that produced the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    /// Generated content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
    /// Grounding metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounding_metadata: Option<GroundingMetadata>,
    /// Citation metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_metadata: Option<CitationMetadata>,
    /// Finish reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    /// Error code (provider-specific).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Set if the model was interrupted mid-stream (bidi).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted: Option<bool>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_metadata: Option<serde_json::Value>,
    /// Usage metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_metadata: Option<UsageMetadata>,
    /// Context-cache usage for this call (set by cache-capable providers
    /// when a [`crate::core::ContextCacheConfig`] is active).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_metadata: Option<crate::core::cache::CacheMetadata>,
}

impl LlmResponse {
    /// Build from a provider-side [`GenerateContentResponse`].
    pub fn from_generate(resp: GenerateContentResponse) -> Self {
        let usage = resp.usage_metadata;
        let model_version = resp.model_version;
        if let Some(candidate) = resp.candidates.into_iter().next() {
            let Candidate {
                content,
                finish_reason,
                finish_message,
                grounding_metadata,
                citation_metadata,
                ..
            } = candidate;
            let has_content = content.as_ref().is_some_and(|c| !c.parts.is_empty());
            if has_content || finish_reason == Some(FinishReason::Stop) {
                return Self {
                    model_version,
                    content,
                    grounding_metadata,
                    citation_metadata,
                    finish_reason,
                    usage_metadata: usage,
                    ..Self::default()
                };
            }
            return Self {
                model_version,
                error_code: finish_reason.map(|r| format!("{r:?}")),
                error_message: finish_message,
                citation_metadata,
                finish_reason,
                usage_metadata: usage,
                ..Self::default()
            };
        }
        if let Some(pf) = resp.prompt_feedback {
            return Self {
                model_version,
                error_code: pf.block_reason,
                error_message: pf.block_reason_message,
                usage_metadata: usage,
                ..Self::default()
            };
        }
        Self {
            model_version,
            error_code: Some("UNKNOWN_ERROR".into()),
            error_message: Some("Unknown error.".into()),
            usage_metadata: usage,
            ..Self::default()
        }
    }

    /// Convenience: was this an error response?
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.error_code.is_some()
    }

    /// Convenience: extract function calls.
    #[must_use]
    pub fn function_calls(&self) -> Vec<FunctionCall> {
        self.content
            .as_ref()
            .map(|c| {
                c.parts
                    .iter()
                    .filter_map(|p| p.as_function_call().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Convenience: extract function responses.
    #[must_use]
    pub fn function_responses(&self) -> Vec<FunctionResponse> {
        self.content
            .as_ref()
            .map(|c| {
                c.parts
                    .iter()
                    .filter_map(|p| p.as_function_response().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_types::Role;
    use crate::genai_types::part::Part;

    #[test]
    fn from_generate_picks_first_candidate() {
        let r = GenerateContentResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Role::Model,
                    parts: vec![Part::text("ok")],
                }),
                finish_reason: Some(FinishReason::Stop),
                finish_message: None,
                safety_ratings: vec![],
                grounding_metadata: None,
                citation_metadata: None,
                avg_logprobs: None,
            }],
            ..Default::default()
        };
        let r = LlmResponse::from_generate(r);
        assert!(!r.is_error());
        assert_eq!(r.content.as_ref().unwrap().text_concat(), "ok");
    }
}
