//! Converters between ADK's native types ([`Content`], [`Event`],
//! [`Part`](crate::genai_types::Part)) and the A2A spec types
//! ([`a2a::Message`](crate::a2a::types::Message),
//! [`a2a::Artifact`](crate::a2a::types::Artifact),
//! [`a2a::Part`](crate::a2a::types::Part)).
//!
//! These mappings are intentionally lossy: A2A's wire shape is the smaller
//! of the two surfaces, so things like `FunctionCall` / `FunctionResponse`
//! parts get serialised into `Part::Data` blobs rather than dropped.

use crate::core::Event;
use crate::genai_types::{Content, Part as AdkPart, Role};

use super::types::{
    Artifact, Message, MessageKind, MessageRole, Part as A2aPart, Task, TaskKind, TaskState,
    TaskStatus,
};

/// Translate an ADK [`Content`] into an A2A [`Message`]. The
/// `context_id` / `task_id` come from the caller because they're set at the
/// envelope level, not on the content itself.
#[must_use]
pub fn content_to_message(
    content: &Content,
    context_id: Option<String>,
    task_id: Option<String>,
) -> Message {
    let role = match content.role {
        Role::User => MessageRole::User,
        Role::Model | Role::Tool | Role::System => MessageRole::Agent,
    };
    let parts = content.parts.iter().map(adk_part_to_a2a).collect();
    Message {
        kind: MessageKind::Message,
        role,
        parts,
        message_id: uuid::Uuid::new_v4().to_string(),
        task_id,
        context_id,
        reference_task_ids: Vec::new(),
        metadata: None,
    }
}

/// Translate an A2A [`Message`] into an ADK [`Content`]. Spec roles map:
/// `"user"` → [`Role::User`], `"agent"` → [`Role::Model`].
#[must_use]
pub fn message_to_content(msg: &Message) -> Content {
    let role = match msg.role {
        MessageRole::User => Role::User,
        MessageRole::Agent => Role::Model,
    };
    Content {
        role,
        parts: msg.parts.iter().map(a2a_part_to_adk).collect(),
    }
}

/// Translate one ADK [`AdkPart`] into one A2A [`A2aPart`]. Non-textual
/// variants (function call/response, inline binary, code) get encoded as
/// [`A2aPart::Data`] so no information is silently dropped — the receiving
/// side can inspect the JSON shape if it cares.
#[must_use]
pub fn adk_part_to_a2a(p: &AdkPart) -> A2aPart {
    match p {
        AdkPart::Text(t) | AdkPart::Thought(t) => A2aPart::text(t.clone()),
        AdkPart::FunctionCall(fc) => A2aPart::Data {
            data: serde_json::json!({"adk:functionCall": fc}),
            metadata: None,
        },
        AdkPart::FunctionResponse(fr) => A2aPart::Data {
            data: serde_json::json!({"adk:functionResponse": fr}),
            metadata: None,
        },
        AdkPart::InlineData(d) => A2aPart::File {
            file: super::types::FilePayload::Inline {
                name: d.display_name.clone(),
                mime_type: Some(d.mime_type.clone()),
                bytes: d.data.clone(),
            },
            metadata: None,
        },
        AdkPart::FileData(d) => A2aPart::File {
            file: super::types::FilePayload::Uri {
                name: d.display_name.clone(),
                mime_type: Some(d.mime_type.clone()),
                uri: d.file_uri.clone(),
            },
            metadata: None,
        },
        AdkPart::ExecutableCode(c) => A2aPart::Data {
            data: serde_json::json!({"adk:executableCode": c}),
            metadata: None,
        },
        AdkPart::CodeExecutionResult(r) => A2aPart::Data {
            data: serde_json::json!({"adk:codeExecutionResult": r}),
            metadata: None,
        },
    }
}

/// Inverse of [`adk_part_to_a2a`]. Unknown `Data` shapes fall through as a
/// JSON-stringified text part so callers can still log them.
#[must_use]
pub fn a2a_part_to_adk(p: &A2aPart) -> AdkPart {
    match p {
        A2aPart::Text { text, .. } => AdkPart::Text(text.clone()),
        A2aPart::File { file, .. } => match file {
            super::types::FilePayload::Inline {
                name,
                mime_type,
                bytes,
            } => AdkPart::InlineData(crate::genai_types::InlineData {
                mime_type: mime_type.clone().unwrap_or_default(),
                data: bytes.clone(),
                display_name: name.clone(),
            }),
            super::types::FilePayload::Uri {
                name,
                mime_type,
                uri,
            } => AdkPart::FileData(crate::genai_types::FileData {
                mime_type: mime_type.clone().unwrap_or_default(),
                file_uri: uri.clone(),
                display_name: name.clone(),
            }),
        },
        A2aPart::Data { data, .. } => {
            // Round-trip via known wrapper keys when present.
            if let Some(fc) = data.get("adk:functionCall") {
                if let Ok(fc) = serde_json::from_value(fc.clone()) {
                    return AdkPart::FunctionCall(fc);
                }
            }
            if let Some(fr) = data.get("adk:functionResponse") {
                if let Ok(fr) = serde_json::from_value(fr.clone()) {
                    return AdkPart::FunctionResponse(fr);
                }
            }
            if let Some(ec) = data.get("adk:executableCode") {
                if let Ok(ec) = serde_json::from_value(ec.clone()) {
                    return AdkPart::ExecutableCode(ec);
                }
            }
            if let Some(r) = data.get("adk:codeExecutionResult") {
                if let Ok(r) = serde_json::from_value(r.clone()) {
                    return AdkPart::CodeExecutionResult(r);
                }
            }
            AdkPart::Text(data.to_string())
        }
    }
}

/// Convert an ADK [`Event`] into an A2A [`Message`], if it carries any
/// content. Returns `None` for events with no content (e.g. pure
/// `actions`-only events).
#[must_use]
pub fn event_to_message(event: &Event, context_id: &str, task_id: &str) -> Option<Message> {
    let content = event.response.content.as_ref()?;
    if content.parts.is_empty() {
        return None;
    }
    Some(content_to_message(
        content,
        Some(context_id.to_string()),
        Some(task_id.to_string()),
    ))
}

/// Build a streaming [`Artifact`] chunk for an agent text event. Used by
/// the server to emit `TaskArtifactUpdateEvent` deltas as text streams in.
#[must_use]
pub fn artifact_chunk(artifact_id: &str, text: &str, append: bool, last_chunk: bool) -> Artifact {
    Artifact {
        artifact_id: artifact_id.to_string(),
        name: None,
        description: None,
        parts: vec![A2aPart::text(text)],
        index: None,
        append: Some(append),
        last_chunk: Some(last_chunk),
        metadata: None,
    }
}

/// Convenience: produce an initial `Submitted` task object for a new
/// `message/send`.
#[must_use]
pub fn new_task(task_id: String, context_id: String, history: Vec<Message>) -> Task {
    Task {
        kind: TaskKind::Task,
        id: task_id,
        context_id,
        status: TaskStatus {
            state: TaskState::Submitted,
            message: None,
            timestamp: Some(crate::a2a::task_service::rfc3339_now()),
        },
        artifacts: vec![],
        history,
        metadata: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_types::FunctionCall;
    use serde_json::json;

    #[test]
    fn text_part_round_trips_unchanged() {
        let adk = AdkPart::Text("hi".into());
        let a2a = adk_part_to_a2a(&adk);
        match &a2a {
            A2aPart::Text { text, .. } => assert_eq!(text, "hi"),
            other => panic!("expected Text, got {other:?}"),
        }
        let back = a2a_part_to_adk(&a2a);
        assert_eq!(back, adk);
    }

    #[test]
    fn function_call_round_trips_through_data() {
        let adk = AdkPart::FunctionCall(FunctionCall::new("f", json!({"x": 1})).with_id("c1"));
        let a2a = adk_part_to_a2a(&adk);
        match &a2a {
            A2aPart::Data { data, .. } => assert!(data.get("adk:functionCall").is_some()),
            other => panic!("expected Data, got {other:?}"),
        }
        let back = a2a_part_to_adk(&a2a);
        assert_eq!(back, adk);
    }

    #[test]
    fn user_content_maps_to_user_message() {
        let c = Content::user_text("hello");
        let m = content_to_message(&c, Some("ctx".into()), None);
        assert_eq!(m.role, MessageRole::User);
        assert_eq!(m.context_id.as_deref(), Some("ctx"));
        let back = message_to_content(&m);
        assert_eq!(back.role, Role::User);
        assert_eq!(back.text_concat(), "hello");
    }

    #[test]
    fn agent_role_maps_back_to_model() {
        let m = Message::agent_text("ok");
        let c = message_to_content(&m);
        assert_eq!(c.role, Role::Model);
    }
}
