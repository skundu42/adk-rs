//! Dev HTTP server (axum) for adk-rs.
//!
//! Provides REST + SSE endpoints around a configured [`Runner`].

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod app;
mod routes;

pub use app::{AppState, build_router, serve};
