//! Dev HTTP server (axum) for adk-rs.
//!
//! Provides REST + SSE endpoints around a configured [`Runner`].


mod app;
mod routes;

pub use app::{AppState, build_router, serve};
