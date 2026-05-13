//! Crate-wide [`Error`] and [`Result`] types for the adk-rs workspace.
//!
//! All public APIs return [`Result<T>`]. Subsystem errors wrap richer
//! per-subsystem types so call sites keep context while consumers see a
//! single error type.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::fmt;

/// Convenience alias for `Result<T, Error>` used throughout the workspace.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The top-level error type for adk-rs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Error originating from an LLM provider client (HTTP, decoding, etc).
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Error originating from a tool invocation.
    #[error(transparent)]
    Tool(#[from] ToolError),
    /// Error originating from a service (sessions, artifacts, memory, ...).
    #[error(transparent)]
    Service(#[from] ServiceError),
    /// Error in schema generation, sanitization, or validation.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// Invariant that should never fail at runtime did.
    #[error("invariant violated: {0}")]
    Invariant(&'static str),
    /// The operation was cancelled.
    #[error("operation cancelled")]
    Cancelled,
    /// Invalid configuration supplied by the caller.
    #[error("invalid configuration: {0}")]
    Config(String),
    /// The requested entity was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// The entity already exists.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// Input validation failure (e.g. malformed args).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// A generic I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON encode/decode failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Any other error, captured as a string.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Construct a misuse / configuration error.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Construct a not-found error.
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    /// Construct an already-exists error.
    pub fn already_exists(msg: impl Into<String>) -> Self {
        Self::AlreadyExists(msg.into())
    }

    /// Construct an invalid-input error.
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }

    /// Construct a generic error.
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

/// Errors from LLM provider clients.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// HTTP transport failure (DNS, TLS, connection, etc).
    #[error("provider transport error: {0}")]
    Transport(String),
    /// Non-2xx HTTP response from the provider.
    #[error("provider returned status {status}: {body}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated if very large).
        body: String,
    },
    /// Provider response was 2xx but the body could not be decoded.
    #[error("could not decode provider response: {0}")]
    Decode(String),
    /// Authentication failed (missing API key, bad credentials).
    #[error("provider authentication error: {0}")]
    Auth(String),
    /// Provider rate limit exceeded.
    #[error("provider rate limit: {0}")]
    RateLimit(String),
    /// Streaming protocol violation.
    #[error("provider streaming error: {0}")]
    Stream(String),
    /// Provider does not support a requested feature.
    #[error("unsupported provider feature: {0}")]
    Unsupported(&'static str),
}

/// Errors from tool invocation.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The arguments did not match the tool schema.
    #[error("tool {tool} got invalid args: {message}")]
    InvalidArgs {
        /// Tool name.
        tool: String,
        /// Validation message.
        message: String,
    },
    /// The tool returned an error while running.
    #[error("tool {tool} execution failed: {message}")]
    Execution {
        /// Tool name.
        tool: String,
        /// Failure message.
        message: String,
    },
    /// The tool requested abort of the entire agent turn.
    #[error("tool {tool} aborted invocation: {message}")]
    Aborted {
        /// Tool name.
        tool: String,
        /// Reason for abort.
        message: String,
    },
    /// Tool of this name is not registered with the agent.
    #[error("unknown tool: {0}")]
    Unknown(String),
}

/// Errors from services (session/artifact/memory/credential).
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// Session not found.
    #[error("session not found: {0}")]
    SessionNotFound(String),
    /// Artifact not found.
    #[error("artifact not found: {0}")]
    ArtifactNotFound(String),
    /// Stale session — concurrent writer detected.
    #[error("stale session: {0}")]
    StaleSession(String),
    /// Database / storage backend failure.
    #[error("storage backend error: {0}")]
    Backend(String),
}

/// Errors from schema generation, sanitization, or validation.
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    /// The schema could not be sanitized for the target provider.
    #[error("schema could not be sanitized: {0}")]
    Sanitize(String),
    /// The schema is invalid.
    #[error("invalid schema: {0}")]
    Invalid(String),
}

/// Convenience trait to add ad-hoc context to an error.
pub trait Context<T> {
    /// Wrap the error with the given context message, returning an
    /// [`Error::Other`] on the error path.
    fn context<C: fmt::Display>(self, ctx: C) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context<C: fmt::Display>(self, ctx: C) -> Result<T> {
        self.map_err(|e| Error::Other(format!("{ctx}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_variants_render() {
        let e: Error = ProviderError::Http {
            status: 500,
            body: "boom".into(),
        }
        .into();
        assert!(e.to_string().contains("500"));
        assert!(e.to_string().contains("boom"));
    }

    #[test]
    fn context_wraps_string_errors() {
        let r: std::result::Result<(), String> = Err("inner".to_string());
        let e = r.context("outer").unwrap_err();
        assert_eq!(e.to_string(), "outer: inner");
    }

    #[test]
    fn constructors_compose() {
        let e = Error::not_found("session sess-1");
        assert!(matches!(e, Error::NotFound(_)));
        assert!(e.to_string().contains("sess-1"));
    }
}
