//! The role of an actor producing [`Content`](crate::genai_types::Content).

use serde::{Deserialize, Serialize};

/// The role a [`Content`](crate::genai_types::Content) is attributed to.
///
/// Serializes as the lowercase string Python ADK expects (`"user"`, `"model"`,
/// `"system"`, `"tool"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The end user.
    User,
    /// The model / assistant.
    Model,
    /// System instructions (rarely used in `Content`; usually a separate field).
    System,
    /// A tool / function-response producer.
    Tool,
}

impl Role {
    /// String form used over the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Model => "model",
            Self::System => "system",
            Self::Tool => "tool",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_via_json() {
        for r in [Role::User, Role::Model, Role::System, Role::Tool] {
            let j = serde_json::to_string(&r).unwrap();
            let back: Role = serde_json::from_str(&j).unwrap();
            assert_eq!(r, back);
        }
    }

    #[test]
    fn serialises_lowercase() {
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
        assert_eq!(serde_json::to_string(&Role::Model).unwrap(), "\"model\"");
    }
}
