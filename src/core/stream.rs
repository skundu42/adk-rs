//! Stream type aliases used throughout adk-rs.

use std::pin::Pin;

use futures::Stream;

use crate::error::Result;

use crate::core::event::Event;
use crate::core::llm_response::LlmResponse;

/// Owned, dynamically-typed stream of events produced by an agent or runner.
pub type EventStream<'a> = Pin<Box<dyn Stream<Item = Result<Event>> + Send + 'a>>;

/// Owned stream of LLM response chunks.
pub type LlmResponseStream = Pin<Box<dyn Stream<Item = Result<LlmResponse>> + Send>>;
