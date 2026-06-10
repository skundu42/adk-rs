//! Python-ADK wire format for the dev server.
//!
//! The adk-web Angular UI (and `google-adk`'s `api_server` clients) speak a
//! camelCase JSON dialect: pydantic models serialized `by_alias=True,
//! exclude_none=True`. The `genai_types` (Content/Part/usage/grounding)
//! already serialize in that wire format; this module maps the remaining
//! ADK-level objects — [`Event`], [`EventActions`], [`Session`] — explicitly,
//! field by field. Explicit construction (rather than generic key renaming)
//! guarantees user data (state keys, tool args) is never rewritten.

use serde_json::{Map, Value, json};

use crate::core::{Event, EventActions, Session, SessionMeta};

fn put(m: &mut Map<String, Value>, key: &str, v: Value) {
    if !v.is_null() {
        m.insert(key.to_string(), v);
    }
}

fn put_ser<T: serde::Serialize>(m: &mut Map<String, Value>, key: &str, v: &Option<T>) {
    if let Some(v) = v {
        put(m, key, serde_json::to_value(v).unwrap_or(Value::Null));
    }
}

/// Serialize an [`Event`] in the Python `api_server` wire shape.
#[must_use]
pub fn event_to_wire(e: &Event) -> Value {
    let mut m = Map::new();
    // Flattened LlmResponse fields.
    put_ser(&mut m, "content", &e.response.content);
    put_ser(&mut m, "modelVersion", &e.response.model_version);
    put_ser(&mut m, "groundingMetadata", &e.response.grounding_metadata);
    put_ser(&mut m, "citationMetadata", &e.response.citation_metadata);
    put_ser(&mut m, "finishReason", &e.response.finish_reason);
    put_ser(&mut m, "errorCode", &e.response.error_code);
    put_ser(&mut m, "errorMessage", &e.response.error_message);
    put_ser(&mut m, "interrupted", &e.response.interrupted);
    put_ser(&mut m, "customMetadata", &e.response.custom_metadata);
    put_ser(&mut m, "usageMetadata", &e.response.usage_metadata);
    if let Some(cm) = &e.response.cache_metadata {
        put(
            &mut m,
            "cacheMetadata",
            json!({ "cacheName": cm.cache_name, "cacheHit": cm.cache_hit }),
        );
    }
    // Event-level fields. Python always emits id/invocationId/author/
    // timestamp/actions (they have non-None defaults).
    m.insert("id".into(), json!(e.id));
    m.insert("invocationId".into(), json!(e.invocation_id));
    m.insert("author".into(), json!(e.author));
    m.insert("timestamp".into(), json!(e.timestamp));
    put_ser(&mut m, "branch", &e.branch);
    put_ser(&mut m, "partial", &e.partial);
    put_ser(&mut m, "turnComplete", &e.turn_complete);
    put_ser(&mut m, "longRunningToolIds", &e.long_running_tool_ids);
    m.insert("actions".into(), actions_to_wire(&e.actions));
    Value::Object(m)
}

fn actions_to_wire(a: &EventActions) -> Value {
    let mut m = Map::new();
    // These four maps are always present in Python (default {}); the UI
    // reads them unconditionally.
    m.insert(
        "stateDelta".into(),
        serde_json::to_value(&a.state_delta).unwrap_or_else(|_| json!({})),
    );
    m.insert(
        "artifactDelta".into(),
        serde_json::to_value(&a.artifact_delta).unwrap_or_else(|_| json!({})),
    );
    m.insert("requestedAuthConfigs".into(), json!({}));
    m.insert(
        "requestedToolConfirmations".into(),
        serde_json::to_value(&a.requested_tool_confirmations).unwrap_or_else(|_| json!({})),
    );
    put_ser(&mut m, "skipSummarization", &a.skip_summarization);
    put_ser(&mut m, "transferToAgent", &a.transfer_to_agent);
    put_ser(&mut m, "escalate", &a.escalate);
    put_ser(&mut m, "endOfAgent", &a.end_of_agent);
    put_ser(&mut m, "agentState", &a.agent_state);
    put_ser(
        &mut m,
        "rewindBeforeInvocationId",
        &a.rewind_before_invocation_id,
    );
    if let Some(c) = &a.compaction {
        put(
            &mut m,
            "compaction",
            json!({
                "startTimestamp": c.start_timestamp,
                "endTimestamp": c.end_timestamp,
                "compactedContent": serde_json::to_value(&c.compacted_content)
                    .unwrap_or(Value::Null),
            }),
        );
    }
    Value::Object(m)
}

/// Serialize a [`Session`] in the Python `api_server` wire shape.
#[must_use]
pub fn session_to_wire(s: &Session) -> Value {
    json!({
        "id": s.id,
        "appName": s.app_name,
        "userId": s.user_id,
        "state": serde_json::to_value(&s.state).unwrap_or_else(|_| json!({})),
        "events": s.events.iter().map(event_to_wire).collect::<Vec<_>>(),
        "lastUpdateTime": s.last_update_time,
    })
}

/// Serialize a [`SessionMeta`] as a Session-shaped wire object (list APIs
/// return sessions without state/events, like Python's `list_sessions`).
#[must_use]
pub fn session_meta_to_wire(s: &SessionMeta) -> Value {
    json!({
        "id": s.id,
        "appName": s.app_name,
        "userId": s.user_id,
        "state": {},
        "events": [],
        "lastUpdateTime": s.last_update_time,
    })
}

/// Best-effort decode of a wire-format event (used for session import).
/// Maps the fields the dev UI round-trips; unknown fields are ignored.
#[must_use]
pub fn event_from_wire(v: &Value) -> Option<Event> {
    let content = v
        .get("content")
        .cloned()
        .and_then(|c| serde_json::from_value(c).ok());
    let author = v.get("author")?.as_str()?.to_string();
    let mut e = Event::new(
        author,
        crate::core::LlmResponse {
            content,
            ..Default::default()
        },
    );
    if let Some(id) = v.get("id").and_then(Value::as_str) {
        e.id = id.to_string();
    }
    if let Some(inv) = v.get("invocationId").and_then(Value::as_str) {
        e.invocation_id = inv.to_string();
    }
    if let Some(ts) = v.get("timestamp").and_then(Value::as_f64) {
        e.timestamp = ts;
    }
    if let Some(delta) = v
        .pointer("/actions/stateDelta")
        .and_then(|d| serde_json::from_value(d.clone()).ok())
    {
        e.actions.state_delta = delta;
    }
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::LlmResponse;
    use crate::genai_types::{Content, FunctionCall, Part, Role};

    #[test]
    fn event_wire_shape_matches_python_contract() {
        let mut e = Event::new(
            "root_agent",
            LlmResponse {
                content: Some(Content {
                    role: Role::Model,
                    parts: vec![
                        Part::text("Hi"),
                        Part::FunctionCall(
                            FunctionCall::new("get_weather", serde_json::json!({"city": "SF"}))
                                .with_id("adk-1"),
                        ),
                    ],
                }),
                ..Default::default()
            },
        );
        e.invocation_id = "inv-1".into();
        e.actions.transfer_to_agent = Some("sub".into());
        e.actions
            .state_delta
            .insert("user:pref".into(), serde_json::json!("dark"));

        let w = event_to_wire(&e);
        assert_eq!(w["invocationId"], "inv-1");
        assert_eq!(w["author"], "root_agent");
        assert_eq!(w["content"]["role"], "model");
        assert_eq!(w["content"]["parts"][0]["text"], "Hi");
        assert_eq!(w["content"]["parts"][1]["functionCall"]["name"], "get_weather");
        assert_eq!(w["actions"]["transferToAgent"], "sub");
        assert_eq!(w["actions"]["stateDelta"]["user:pref"], "dark");
        // Always-present maps.
        assert!(w["actions"]["artifactDelta"].is_object());
        assert!(w["actions"]["requestedAuthConfigs"].is_object());
        assert!(w["actions"]["requestedToolConfirmations"].is_object());
        // Nulls are omitted, not emitted.
        assert!(w.get("errorCode").is_none());
        assert!(w.get("branch").is_none());
        assert!(w["timestamp"].is_f64());
    }

    #[test]
    fn session_wire_shape() {
        let mut s = Session::new("my_app", "alice", "s-1");
        s.events.push(Event::user_text("hello"));
        let w = session_to_wire(&s);
        assert_eq!(w["appName"], "my_app");
        assert_eq!(w["userId"], "alice");
        assert_eq!(w["events"][0]["author"], "user");
        assert!(w["lastUpdateTime"].is_f64());
    }

    #[test]
    fn event_round_trips_from_wire() {
        let e = Event::model_text("a", "hi");
        let w = event_to_wire(&e);
        let back = event_from_wire(&w).unwrap();
        assert_eq!(back.id, e.id);
        assert_eq!(
            back.response.content.as_ref().unwrap().text_concat(),
            "hi"
        );
    }
}
