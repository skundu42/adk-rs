//! Artifact key and metadata types.

use serde::{Deserialize, Serialize};

use adk_rs_genai_types::Part;

/// A versioned artifact stored under `(app, user, session, filename)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactKey {
    /// App name.
    pub app_name: String,
    /// User id.
    pub user_id: String,
    /// Session id.
    pub session_id: String,
    /// Filename (may include `/` for hierarchical layout).
    pub filename: String,
}

impl ArtifactKey {
    /// Construct.
    pub fn new(
        app_name: impl Into<String>,
        user_id: impl Into<String>,
        session_id: impl Into<String>,
        filename: impl Into<String>,
    ) -> Self {
        Self {
            app_name: app_name.into(),
            user_id: user_id.into(),
            session_id: session_id.into(),
            filename: filename.into(),
        }
    }
}

/// An artifact bundle: a [`Part`] (typically `InlineData`/`FileData`/`Text`)
/// and its version number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    /// The stored part.
    pub part: Part,
    /// Version (1-indexed; monotonically increasing per key).
    pub version: u64,
}
