//! Dev HTTP server (axum) for adk-rs.
//!
//! Provides REST + SSE endpoints around a configured [`crate::runner::Runner`].

mod app;
mod routes;

pub use app::{AppState, build_router, serve};
