//! Event model — the unit appended to a session.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::genai_types::{Content, FunctionCall, FunctionResponse, Part};

use crate::core::llm_response::LlmResponse;
use crate::core::state::StateDelta;

/// Actions attached to an [`Event`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventActions {
    /// If `Some(true)`, the runner skips summarization of the function
    /// response (per Python `EventActions.skip_summarization`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_summarization: Option<bool>,
    /// State delta to apply on append.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub state_delta: StateDelta,
    /// Artifact-version delta: filename → new version.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub artifact_delta: IndexMap<String, u64>,
    /// If set, the runner transfers control to the named agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_to_agent: Option<String>,
    /// Whether the agent is escalating control upward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalate: Option<bool>,
    /// Whether the current agent has finished its run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_of_agent: Option<bool>,
    /// Compaction info, if this event compacted earlier events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<EventCompaction>,
    /// Free-form agent-checkpoint state for resumption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_state: Option<serde_json::Value>,
    /// Invocation id to rewind to (for rewind events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewind_before_invocation_id: Option<String>,
}

/// Compaction info attached to a summary event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventCompaction {
    /// Timestamp of the earliest compacted event (seconds).
    pub start_timestamp: f64,
    /// Timestamp of the latest compacted event (seconds).
    pub end_timestamp: f64,
    /// The replacement content (typically a summary).
    pub compacted_content: Content,
}

/// A single event in a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Unique id for this event (auto-assigned).
    #[serde(default)]
    pub id: String,
    /// Invocation id the event belongs to.
    #[serde(default)]
    pub invocation_id: String,
    /// Author: `"user"` or the agent name.
    pub author: String,
    /// Wall-clock timestamp in seconds.
    #[serde(default)]
    pub timestamp: f64,
    /// Optional agent-tree branch (e.g. `parent.child`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Underlying LLM-style response payload (content + finish reason + ...).
    #[serde(flatten)]
    pub response: LlmResponse,
    /// Attached actions.
    #[serde(default, skip_serializing_if = "is_default_event_actions")]
    pub actions: EventActions,
    /// Ids of long-running tool calls associated with this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_running_tool_ids: Option<Vec<String>>,
    /// True if this event is a partial streaming chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<bool>,
    /// True if this event ends a streaming turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_complete: Option<bool>,
}

fn is_default_event_actions(a: &EventActions) -> bool {
    *a == EventActions::default()
}

impl Event {
    /// Make a new id.
    #[must_use]
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Build an event with a fresh id, current timestamp, and the given author + response.
    pub fn new(author: impl Into<String>, response: LlmResponse) -> Self {
        Self {
            id: Self::new_id(),
            invocation_id: String::new(),
            author: author.into(),
            timestamp: crate::core::session::now_secs(),
            branch: None,
            response,
            actions: EventActions::default(),
            long_running_tool_ids: None,
            partial: None,
            turn_complete: None,
        }
    }

    /// Build a user event from text.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::new(
            "user",
            LlmResponse {
                content: Some(Content::user_text(text)),
                ..LlmResponse::default()
            },
        )
    }

    /// Build a model-author event from text.
    pub fn model_text(author: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(
            author,
            LlmResponse {
                content: Some(Content::model_text(text)),
                ..LlmResponse::default()
            },
        )
    }

    /// Returns function calls in the event content, if any.
    #[must_use]
    pub fn function_calls(&self) -> Vec<FunctionCall> {
        self.response
            .content
            .as_ref()
            .map(|c| {
                c.parts
                    .iter()
                    .filter_map(|p| p.as_function_call().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns function responses in the event content, if any.
    #[must_use]
    pub fn function_responses(&self) -> Vec<FunctionResponse> {
        self.response
            .content
            .as_ref()
            .map(|c| {
                c.parts
                    .iter()
                    .filter_map(|p| p.as_function_response().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns whether this event ends an agent's response (mirrors Python
    /// `Event.is_final_response`).
    #[must_use]
    pub fn is_final_response(&self) -> bool {
        if self.actions.skip_summarization == Some(true)
            || self
                .long_running_tool_ids
                .as_ref()
                .is_some_and(|ids| !ids.is_empty())
        {
            return true;
        }
        let has_calls = !self.function_calls().is_empty();
        let has_responses = !self.function_responses().is_empty();
        !has_calls
            && !has_responses
            && self.partial != Some(true)
            && !self.has_trailing_code_result()
    }

    /// True if the last part is a code-execution result.
    #[must_use]
    pub fn has_trailing_code_result(&self) -> bool {
        self.response
            .content
            .as_ref()
            .and_then(|c| c.parts.last())
            .is_some_and(|p| matches!(p, Part::CodeExecutionResult(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_types::FunctionCall;
    use serde_json::json;

    #[test]
    fn user_text_is_final() {
        let e = Event::user_text("hi");
        assert!(e.is_final_response());
    }

    #[test]
    fn event_with_function_call_is_not_final() {
        let resp = LlmResponse {
            content: Some(Content {
                role: crate::genai_types::Role::Model,
                parts: vec![Part::FunctionCall(FunctionCall::new("f", json!({})))],
            }),
            ..LlmResponse::default()
        };
        let e = Event::new("agent", resp);
        assert!(!e.is_final_response());
        assert_eq!(e.function_calls().len(), 1);
    }

    #[test]
    fn event_round_trips() {
        let e = Event::model_text("agent", "hello");
        let j = serde_json::to_value(&e).unwrap();
        let back: Event = serde_json::from_value(j).unwrap();
        assert_eq!(e.id, back.id);
        assert_eq!(e.author, back.author);
    }
}
