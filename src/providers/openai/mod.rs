//! OpenAI-compatible provider for adk-rs (also handles Azure / Ollama / Groq
//! via base-URL override).

mod client;
mod convert;
mod embedder;

pub use client::{OpenAi, OpenAiConfig};
pub use embedder::OpenAiEmbedder;
