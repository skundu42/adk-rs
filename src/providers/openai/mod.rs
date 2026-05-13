//! OpenAI-compatible provider for adk-rs (also handles Azure / Ollama / Groq
//! via base-URL override).

mod client;
mod convert;

pub use client::{OpenAi, OpenAiConfig};
