//! Anthropic Claude provider for adk-rs.

mod client;
mod convert;

pub use client::{Anthropic, AnthropicConfig};

use crate::core::LlmResponse;
use crate::core::stream::LlmResponseStream;

pub(crate) fn stream_one(r: LlmResponse) -> LlmResponseStream {
    use futures::stream;
    Box::pin(stream::once(async move { Ok::<_, crate::error::Error>(r) }))
}
