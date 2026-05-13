//! OpenAI-compatible provider for adk-rs (also handles Azure / Ollama / Groq
//! via base-URL override).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod client;
mod convert;

pub use client::{OpenAi, OpenAiConfig};
