//! LLM providers. Each provider is gated behind its own cargo feature.

#[cfg(any(feature = "gemini", feature = "anthropic", feature = "openai"))]
pub(crate) mod common;

#[cfg(feature = "gemini")]
pub mod gemini;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;
