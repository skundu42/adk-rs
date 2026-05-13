//! [`Content`] — a (role, parts) pair, the top-level unit exchanged with LLMs.

use serde::{Deserialize, Serialize};

use crate::part::Part;
use crate::role::Role;

/// A single content item: a role plus an ordered list of parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// The role this content is attributed to.
    pub role: Role,
    /// The parts (text, function calls, etc.) carried by the content.
    #[serde(default)]
    pub parts: Vec<Part>,
}

impl Content {
    /// Construct a user message of a single text part.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            parts: vec![Part::text(text)],
        }
    }

    /// Construct a model message of a single text part.
    pub fn model_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Model,
            parts: vec![Part::text(text)],
        }
    }

    /// Construct a system message of a single text part.
    pub fn system_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            parts: vec![Part::text(text)],
        }
    }

    /// Concatenate all text parts (and thoughts) into a single string.
    #[must_use]
    pub fn text_concat(&self) -> String {
        let mut out = String::new();
        for p in &self.parts {
            if let Some(t) = p.as_text() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_text_round_trip() {
        let c = Content::user_text("hi");
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["role"], "user");
        assert_eq!(j["parts"][0]["text"], "hi");
        let back: Content = serde_json::from_value(j).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn text_concat_joins() {
        let c = Content {
            role: Role::Model,
            parts: vec![Part::text("a"), Part::text("b")],
        };
        assert_eq!(c.text_concat(), "a\nb");
    }
}
