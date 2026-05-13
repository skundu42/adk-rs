//! Session state, key namespaces, and state deltas.
//!
//! State keys are namespaced with a prefix:
//!
//! * `app:` — scoped to the application (shared across users and sessions).
//! * `user:` — scoped to the user (shared across sessions).
//! * `temp:` — ephemeral; lives for the duration of the current invocation
//!   only and is *not* persisted.
//! * (no prefix) — scoped to the session.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Marker prefixes for state scopes (mirrors Python `state.State`).
pub mod prefix {
    /// Application-scoped state.
    pub const APP: &str = "app:";
    /// User-scoped state.
    pub const USER: &str = "user:";
    /// Temporary, invocation-scoped state. Never persisted.
    pub const TEMP: &str = "temp:";
}

/// The lexical scope a state key belongs to (derived from its prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateScope {
    /// App scope (`app:` prefix).
    App,
    /// User scope (`user:` prefix).
    User,
    /// Temp scope (`temp:` prefix); not persisted.
    Temp,
    /// Session scope (no prefix).
    Session,
}

impl StateScope {
    /// Determine the scope of a key from its prefix.
    #[must_use]
    pub fn of(key: &str) -> Self {
        if key.starts_with(prefix::APP) {
            Self::App
        } else if key.starts_with(prefix::USER) {
            Self::User
        } else if key.starts_with(prefix::TEMP) {
            Self::Temp
        } else {
            Self::Session
        }
    }
}

/// A keyed delta to apply to a [`State`]. Insertion order is preserved.
pub type StateDelta = IndexMap<String, Value>;

/// Mutable session state. Keys may carry a scope prefix.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct State {
    /// Underlying map. Use [`Self::get`] / [`Self::set`] for typed access.
    pub map: IndexMap<String, Value>,
}

impl State {
    /// Construct an empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct from any iterator of `(key, value)`.
    pub fn from_iter<I: IntoIterator<Item = (String, Value)>>(it: I) -> Self {
        Self {
            map: it.into_iter().collect(),
        }
    }

    /// Get a borrowed value by key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    /// Get the scope of a key.
    #[must_use]
    pub fn scope(key: &str) -> StateScope {
        StateScope::of(key)
    }

    /// Set a value. Returns the previous value if any.
    pub fn set(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        self.map.insert(key.into(), value)
    }

    /// Apply a delta. Temp-scoped keys are also stored (callers that
    /// persist must trim them via [`Self::trim_temp_keys`]).
    pub fn apply(&mut self, delta: &StateDelta) {
        for (k, v) in delta {
            self.map.insert(k.clone(), v.clone());
        }
    }

    /// Number of keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the state is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Iterate.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.map.iter()
    }

    /// Remove all `temp:` keys from a delta and return the cleaned delta.
    /// Used when persisting events to durable storage.
    #[must_use]
    pub fn trim_temp_keys(delta: &StateDelta) -> StateDelta {
        delta
            .iter()
            .filter(|(k, _)| StateScope::of(k) != StateScope::Temp)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scope_detection() {
        assert_eq!(StateScope::of("app:foo"), StateScope::App);
        assert_eq!(StateScope::of("user:foo"), StateScope::User);
        assert_eq!(StateScope::of("temp:foo"), StateScope::Temp);
        assert_eq!(StateScope::of("plain"), StateScope::Session);
    }

    #[test]
    fn apply_inserts_keys() {
        let mut s = State::new();
        let mut d = StateDelta::new();
        d.insert("a".into(), json!(1));
        d.insert("temp:b".into(), json!(2));
        s.apply(&d);
        assert_eq!(s.get("a"), Some(&json!(1)));
        assert_eq!(s.get("temp:b"), Some(&json!(2)));
    }

    #[test]
    fn trim_temp_removes_temp_keys() {
        let mut d = StateDelta::new();
        d.insert("a".into(), json!(1));
        d.insert("temp:b".into(), json!(2));
        let t = State::trim_temp_keys(&d);
        assert!(t.contains_key("a"));
        assert!(!t.contains_key("temp:b"));
    }
}
