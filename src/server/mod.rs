//! Dev HTTP server (axum) for adk-rs.
//!
//! Provides REST + SSE endpoints around configured [`crate::runner::Runner`]s,
//! implementing the wire contract of Python ADK's `adk api_server` (camelCase
//! JSON, `/apps/{app}/users/{user}/sessions/...` paths, `data:` SSE framing)
//! so Google's adk-web dev UI and `google-adk` API clients interoperate.

mod adk_web;
mod app;
mod routes;
pub mod wire;

pub use app::{AppState, ServeOptions, build_router, serve, serve_with};
