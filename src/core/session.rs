//! [`Session`] data model: a conversation between a user and one or more agents.

use serde::{Deserialize, Serialize};

use crate::core::event::Event;
use crate::core::state::State;

/// Convenience alias for session identifiers.
pub type SessionId = String;

/// A conversation, owned by a `(app_name, user_id, id)` triple.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Unique id (within `(app_name, user_id)`).
    pub id: SessionId,
    /// App name this session belongs to.
    pub app_name: String,
    /// User id this session belongs to.
    pub user_id: String,
    /// Mutable state map.
    #[serde(default)]
    pub state: State,
    /// All events ever appended to this session (oldest first).
    #[serde(default)]
    pub events: Vec<Event>,
    /// Last update time (seconds since epoch, float).
    #[serde(default)]
    pub last_update_time: f64,
}

impl Session {
    /// Build a brand-new empty session.
    pub fn new(
        app_name: impl Into<String>,
        user_id: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            app_name: app_name.into(),
            user_id: user_id.into(),
            state: State::default(),
            events: Vec::new(),
            last_update_time: now_secs(),
        }
    }
}

/// Metadata about a session without events/state (used by list APIs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Session id.
    pub id: SessionId,
    /// App name.
    pub app_name: String,
    /// User id.
    pub user_id: String,
    /// Last update time (seconds since epoch).
    pub last_update_time: f64,
}

/// Seconds since the unix epoch, as `f64`.
pub fn now_secs() -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Sub-second precision retained.
    let secs = now.as_secs() as f64;
    let nanos = f64::from(now.subsec_nanos()) / 1_000_000_000.0;
    secs + nanos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips() {
        let s = Session::new("a", "u", "sess");
        let j = serde_json::to_value(&s).unwrap();
        let back: Session = serde_json::from_value(j).unwrap();
        assert_eq!(s, back);
    }
}
