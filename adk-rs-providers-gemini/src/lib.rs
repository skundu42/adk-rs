//! Google Gemini REST + SSE provider for adk-rs.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod client;
mod convert;
mod stream;

pub use client::{Gemini, GeminiConfig};
