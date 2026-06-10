//! Tool confirmation (human-in-the-loop) — types and the user-event
//! preprocessor (mirrors Python ADK's `ToolConfirmation` /
//! `adk_request_confirmation` flow, v1.14+).
//!
//! Flow:
//! 1. A tool is registered with `require_confirmation` (see
//!    [`crate::tools::FunctionTool::require_confirmation`] /
//!    [`crate::mcp::McpToolset`]).
//! 2. When the model calls it, the agent does **not** run the tool. It emits
//!    a `FunctionResponse` named [`REQUEST_CONFIRMATION_FUNCTION_NAME`]
//!    carrying a [`ConfirmationRequest`], marks the call long-running, and
//!    pauses (the event also lists the call in
//!    `actions.requested_tool_confirmations`).
//! 3. The caller shows the request to a human and resubmits a
//!    `FunctionResponse` with the same `id`, name
//!    [`REQUEST_CONFIRMATION_FUNCTION_NAME`], and a [`ToolConfirmation`]
//!    (`confirmed: true/false`, optional payload) as the response value.
//! 4. The runner absorbs it via [`ConfirmationPreprocessor`] and the agent
//!    replays the original call — dispatching the tool if confirmed, or
//!    surfacing a rejection result to the model if denied.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::event::Event;
use crate::genai_types::FunctionCall;

/// Name of the synthetic function call used to request tool confirmation.
pub const REQUEST_CONFIRMATION_FUNCTION_NAME: &str = "adk_request_confirmation";

/// A confirmation decision (or request) for one tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolConfirmation {
    /// Human-readable description of what is being confirmed.
    #[serde(default)]
    pub hint: String,
    /// Whether the user approved the tool call.
    #[serde(default)]
    pub confirmed: bool,
    /// Optional structured payload the user supplied along with the
    /// decision (e.g. an amended parameter set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

/// Payload of the synthetic `adk_request_confirmation` response emitted when
/// a tool requires confirmation. Serialized in camelCase for compatibility
/// with Python ADK clients and the adk-web UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmationRequest {
    /// The original function call awaiting confirmation.
    pub original_function_call: FunctionCall,
    /// The confirmation being requested (with `confirmed: false`).
    pub tool_confirmation: ToolConfirmation,
}

/// Outcome of [`ConfirmationPreprocessor::process_event`].
#[derive(Debug, Default)]
pub struct ConfirmationOutcome {
    /// Function-call ids whose deferred tool calls are now unblocked,
    /// mapped to the user's decision.
    pub responses: IndexMap<String, ToolConfirmation>,
}

/// Stateless processor invoked by the runner on each user-authored event;
/// extracts `adk_request_confirmation` responses.
#[derive(Debug, Default)]
pub struct ConfirmationPreprocessor;

impl ConfirmationPreprocessor {
    /// New.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Walk `event`'s function responses for
    /// [`REQUEST_CONFIRMATION_FUNCTION_NAME`] entries and decode the user's
    /// [`ToolConfirmation`] decisions.
    pub fn process_event(&self, event: &Event) -> ConfirmationOutcome {
        let mut out = ConfirmationOutcome::default();
        if event.author != "user" {
            return out;
        }
        let Some(content) = event.response.content.as_ref() else {
            return out;
        };
        for part in &content.parts {
            let crate::genai_types::Part::FunctionResponse(fr) = part else {
                continue;
            };
            if fr.name != REQUEST_CONFIRMATION_FUNCTION_NAME {
                continue;
            }
            let Some(id) = fr.id.clone().filter(|s| !s.is_empty()) else {
                tracing::warn!(
                    "confirmation preprocessor: dropping adk_request_confirmation response \
                     with no function_call_id"
                );
                continue;
            };
            // Accept either a bare ToolConfirmation or a
            // {"toolConfirmation": {...}} / {"response": {...}} wrapper.
            let payload = fr
                .response
                .get("toolConfirmation")
                .or_else(|| fr.response.get("response"))
                .unwrap_or(&fr.response);
            match serde_json::from_value::<ToolConfirmation>(payload.clone()) {
                Ok(c) => {
                    out.responses.insert(id, c);
                }
                Err(e) => {
                    tracing::warn!(
                        "confirmation preprocessor: malformed ToolConfirmation response: {e}"
                    );
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::llm_response::LlmResponse;
    use crate::genai_types::{Content, FunctionResponse, Part, Role};
    use serde_json::json;

    fn user_event(id: &str, response: Value) -> Event {
        Event::new(
            "user",
            LlmResponse {
                content: Some(Content {
                    role: Role::User,
                    parts: vec![Part::FunctionResponse(FunctionResponse {
                        id: Some(id.into()),
                        name: REQUEST_CONFIRMATION_FUNCTION_NAME.into(),
                        response,
                        will_continue: None,
                        scheduling: None,
                    })],
                }),
                ..LlmResponse::default()
            },
        )
    }

    #[test]
    fn absorbs_bare_tool_confirmation() {
        let ev = user_event("fc-1", json!({"confirmed": true, "hint": "ok?"}));
        let out = ConfirmationPreprocessor::new().process_event(&ev);
        assert!(out.responses.get("fc-1").unwrap().confirmed);
    }

    #[test]
    fn absorbs_wrapped_tool_confirmation() {
        let ev = user_event("fc-2", json!({"toolConfirmation": {"confirmed": false}}));
        let out = ConfirmationPreprocessor::new().process_event(&ev);
        assert!(!out.responses.get("fc-2").unwrap().confirmed);
    }

    #[test]
    fn ignores_non_user_events() {
        let mut ev = user_event("fc-3", json!({"confirmed": true}));
        ev.author = "agent".into();
        let out = ConfirmationPreprocessor::new().process_event(&ev);
        assert!(out.responses.is_empty());
    }

    #[test]
    fn confirmation_request_serializes_camel_case() {
        let req = ConfirmationRequest {
            original_function_call: FunctionCall::new("transfer_money", json!({"amount": 5})),
            tool_confirmation: ToolConfirmation {
                hint: "Approve transfer?".into(),
                confirmed: false,
                payload: None,
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("originalFunctionCall").is_some());
        assert_eq!(v["toolConfirmation"]["confirmed"], json!(false));
    }
}
