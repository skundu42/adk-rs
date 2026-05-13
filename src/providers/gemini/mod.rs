//! Google Gemini REST + SSE provider for adk-rs.

mod client;
mod convert;
mod stream;

pub use client::{Gemini, GeminiConfig};
