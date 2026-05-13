//! Server-Sent Events helpers for Gemini streaming.

use futures::stream::StreamExt;

use adk_rs_core::{LlmResponse, LlmResponseStream};
use adk_rs_error::{ProviderError, Result};

use crate::convert;

/// Convert a reqwest streaming body into an [`LlmResponseStream`].
pub(crate) fn from_sse(resp: reqwest::Response) -> LlmResponseStream {
    use eventsource_stream::Eventsource;
    let bytes = resp
        .bytes_stream()
        .map(|r| r.map_err(|e| std::io::Error::other(e.to_string())));
    // `eventsource_stream::Eventsource` wants an `impl Stream<Item = Result<Bytes, std::io::Error>>`.
    let evs = bytes.eventsource();
    let mapped = evs.filter_map(|ev| async move {
        match ev {
            Ok(e) if !e.data.is_empty() => match convert::parse_stream_chunk(&e.data) {
                Ok(r) => Some(Ok(r)),
                Err(err) => Some(Err(err)),
            },
            Ok(_) => None,
            Err(e) => Some(Err(adk_rs_error::Error::Provider(ProviderError::Stream(
                e.to_string(),
            )))),
        }
    });
    Box::pin(mapped) as LlmResponseStream
}

#[allow(dead_code)] // exposed for tests in client.rs
pub(crate) fn boxed_one(r: LlmResponse) -> LlmResponseStream {
    use futures::stream;
    Box::pin(stream::once(async move { Ok::<_, adk_rs_error::Error>(r) }))
}

#[allow(dead_code)]
pub(crate) fn collect_stream(
    s: LlmResponseStream,
) -> impl std::future::Future<Output = Result<Vec<LlmResponse>>> + Send {
    use futures::TryStreamExt;
    s.try_collect::<Vec<_>>()
}
