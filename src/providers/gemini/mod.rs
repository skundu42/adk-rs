//! Google Gemini REST + SSE provider for adk-rs.

mod client;
mod convert;
mod embedder;
#[cfg(feature = "live")]
mod live;
mod stream;

pub use client::{Gemini, GeminiConfig};
pub use embedder::GeminiEmbedder;
#[cfg(feature = "live")]
pub use live::{LiveConfig, LiveEvent, LiveSession};
