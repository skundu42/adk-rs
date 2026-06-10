//! Event compaction — summarizes older session events into a single
//! [`EventCompaction`](crate::core::EventCompaction) so long-running
//! sessions don't grow LLM context without bound (mirrors Python ADK's
//! `EventsCompactionConfig` + `LlmEventSummarizer`).
//!
//! The runner triggers compaction after an invocation completes when at
//! least `compaction_interval` invocations have accumulated since the last
//! compaction. The window includes `overlap_size` already-compacted events
//! for continuity. On the read side,
//! [`crate::core::history_with_compaction`] replaces compacted events with
//! the summary content when agents assemble LLM history.

use std::sync::Arc;

use async_trait::async_trait;

use crate::core::{Event, EventCompaction, LlmRequest, LlmResponse, Model};
use crate::error::Result;
use crate::genai_types::{Content, Part};

/// Produces a summary [`Content`] for a window of events.
#[async_trait]
pub trait EventSummarizer: Send + Sync + std::fmt::Debug + 'static {
    /// Summarize `events`. Return `Ok(None)` to skip compaction (e.g. when
    /// the window has no summarizable content).
    async fn summarize(&self, events: &[Event]) -> Result<Option<Content>>;
}

/// Default summarizer: renders the window as a transcript and asks an LLM
/// for a compact summary.
#[derive(Debug)]
pub struct LlmEventSummarizer {
    model: Arc<dyn Model>,
}

const SUMMARY_INSTRUCTION: &str = "You compact conversation history. Summarize the \
following agent conversation transcript into a concise paragraph that preserves all \
facts, decisions, tool results, and open questions needed to continue the \
conversation. Reply with the summary only.";

impl LlmEventSummarizer {
    /// Construct with the model used to produce summaries.
    pub fn new(model: Arc<dyn Model>) -> Self {
        Self { model }
    }

    fn transcript(events: &[Event]) -> String {
        let mut out = String::new();
        for e in events {
            let Some(content) = e.response.content.as_ref() else {
                continue;
            };
            for part in &content.parts {
                match part {
                    Part::Text(t) if !t.is_empty() => {
                        out.push_str(&format!("{}: {}\n", e.author, t));
                    }
                    Part::FunctionCall(fc) => {
                        out.push_str(&format!("{}: [calls {}({})]\n", e.author, fc.name, fc.args));
                    }
                    Part::FunctionResponse(fr) => {
                        out.push_str(&format!(
                            "{}: [{} returned {}]\n",
                            e.author, fr.name, fr.response
                        ));
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

#[async_trait]
impl EventSummarizer for LlmEventSummarizer {
    async fn summarize(&self, events: &[Event]) -> Result<Option<Content>> {
        let transcript = Self::transcript(events);
        if transcript.is_empty() {
            return Ok(None);
        }
        let mut req = LlmRequest {
            model: Some(self.model.name().to_string()),
            contents: vec![Content::user_text(transcript)],
            ..Default::default()
        };
        req.append_system_text(SUMMARY_INSTRUCTION);
        let resp: LlmResponse = self.model.generate_content(req).await?;
        let Some(text) = resp.content.map(|c| c.text_concat()) else {
            return Ok(None);
        };
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(Content::user_text(format!(
            "[Summary of earlier conversation] {text}"
        ))))
    }
}

/// Configuration for automatic event compaction.
#[derive(Clone)]
pub struct EventsCompactionConfig {
    /// Run compaction after this many invocations have accumulated since the
    /// previous compaction.
    pub compaction_interval: usize,
    /// Number of already-compacted events to re-include at the start of the
    /// next window, for continuity across summaries.
    pub overlap_size: usize,
    /// The summarizer that produces the replacement content.
    pub summarizer: Arc<dyn EventSummarizer>,
}

impl std::fmt::Debug for EventsCompactionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventsCompactionConfig")
            .field("compaction_interval", &self.compaction_interval)
            .field("overlap_size", &self.overlap_size)
            .finish_non_exhaustive()
    }
}

impl EventsCompactionConfig {
    /// Construct with the default LLM summarizer and a default interval of
    /// 5 invocations with 2 events of overlap.
    pub fn new(model: Arc<dyn Model>) -> Self {
        Self {
            compaction_interval: 5,
            overlap_size: 2,
            summarizer: Arc::new(LlmEventSummarizer::new(model)),
        }
    }

    /// Override the compaction interval (in invocations).
    #[must_use]
    pub fn compaction_interval(mut self, n: usize) -> Self {
        self.compaction_interval = n.max(1);
        self
    }

    /// Override the overlap size (in events).
    #[must_use]
    pub fn overlap_size(mut self, n: usize) -> Self {
        self.overlap_size = n;
        self
    }

    /// Use a custom summarizer.
    #[must_use]
    pub fn summarizer(mut self, s: Arc<dyn EventSummarizer>) -> Self {
        self.summarizer = s;
        self
    }
}

/// Decide whether compaction is due and, if so, compute the window to
/// summarize. Pure function over the event log (no locks held by callers
/// while summarizing).
pub(crate) fn compaction_window(
    events: &[Event],
    cfg: &EventsCompactionConfig,
) -> Option<Vec<Event>> {
    let tail_start = events
        .iter()
        .rposition(|e| e.actions.compaction.is_some())
        .map_or(0, |i| i + 1);
    let tail = &events[tail_start..];

    let mut invocations: Vec<&str> = tail
        .iter()
        .filter(|e| !e.invocation_id.is_empty())
        .map(|e| e.invocation_id.as_str())
        .collect();
    invocations.dedup();
    if invocations.len() < cfg.compaction_interval {
        return None;
    }

    // Include `overlap_size` events from before the tail for continuity
    // (skipping prior compaction marker events themselves).
    let mut overlap: Vec<Event> = events[..tail_start]
        .iter()
        .rev()
        .filter(|e| e.actions.compaction.is_none())
        .take(cfg.overlap_size)
        .cloned()
        .collect();
    overlap.reverse();
    let mut window = overlap;
    window.extend(tail.iter().cloned());
    if window.is_empty() {
        None
    } else {
        Some(window)
    }
}

/// Build the compaction marker event for a summarized `window`.
pub(crate) fn compaction_event(author: &str, window: &[Event], summary: Content) -> Event {
    let mut e = Event::new(author, LlmResponse::default());
    e.actions.compaction = Some(EventCompaction {
        start_timestamp: window.first().map(|e| e.timestamp).unwrap_or_default(),
        end_timestamp: window.last().map(|e| e.timestamp).unwrap_or_default(),
        compacted_content: summary,
    });
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::MockModel;

    fn ev(invocation: &str, text: &str, ts: f64) -> Event {
        let mut e = Event::model_text("a", text);
        e.invocation_id = invocation.into();
        e.timestamp = ts;
        e
    }

    #[test]
    fn window_none_below_interval() {
        let model = Arc::new(MockModel::new("m"));
        let cfg = EventsCompactionConfig::new(model).compaction_interval(3);
        let events = vec![ev("i1", "a", 1.0), ev("i2", "b", 2.0)];
        assert!(compaction_window(&events, &cfg).is_none());
    }

    #[test]
    fn window_includes_overlap_and_tail() {
        let model = Arc::new(MockModel::new("m"));
        let cfg = EventsCompactionConfig::new(model)
            .compaction_interval(2)
            .overlap_size(1);
        // Prior compaction at index 2 covering the first two events.
        let mut events = vec![ev("i1", "a", 1.0), ev("i1", "b", 2.0)];
        events.push(compaction_event(
            "a",
            &events.clone(),
            Content::user_text("[s]"),
        ));
        events.push(ev("i2", "c", 3.0));
        events.push(ev("i3", "d", 4.0));
        let w = compaction_window(&events, &cfg).unwrap();
        let texts: Vec<String> = w
            .iter()
            .filter_map(|e| e.response.content.as_ref().map(|c| c.text_concat()))
            .collect();
        // 1 overlap event ("b") + the tail ("c", "d").
        assert_eq!(texts, vec!["b", "c", "d"]);
    }

    #[tokio::test]
    async fn llm_summarizer_produces_prefixed_summary() {
        let model = Arc::new(MockModel::new("m"));
        model.push_text("the gist");
        let s = LlmEventSummarizer::new(model.clone());
        let events = vec![ev("i1", "hello", 1.0)];
        let c = s.summarize(&events).await.unwrap().unwrap();
        assert_eq!(
            c.text_concat(),
            "[Summary of earlier conversation] the gist"
        );
        // The transcript reached the model.
        let reqs = model.captured_requests();
        assert!(reqs[0].contents[0].text_concat().contains("a: hello"));
    }

    #[tokio::test]
    async fn llm_summarizer_skips_empty_window() {
        let model = Arc::new(MockModel::new("m"));
        let s = LlmEventSummarizer::new(model);
        let events = vec![Event::new("a", LlmResponse::default())];
        assert!(s.summarize(&events).await.unwrap().is_none());
    }
}
