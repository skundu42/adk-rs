//! Anthropic Claude provider for adk-rs.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod client;
mod convert;

pub use client::{Anthropic, AnthropicConfig};

use adk_rs_core::LlmResponse;
use adk_rs_core::stream::LlmResponseStream;

pub(crate) fn stream_one(r: LlmResponse) -> LlmResponseStream {
    use futures::stream;
    Box::pin(stream::once(async move { Ok::<_, adk_rs_error::Error>(r) }))
}
