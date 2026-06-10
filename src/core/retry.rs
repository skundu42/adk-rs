//! Retry policy for provider HTTP calls.
//!
//! Long-running agents hit rate limits and transient provider failures as a
//! matter of course. [`RetryConfig`] describes how provider clients retry
//! them: exponential backoff with full jitter, a `Retry-After` header
//! override, and a bounded attempt budget. Every provider config
//! (`GeminiConfig`, `AnthropicConfig`, `OpenAiConfig`) embeds one; the
//! default retries twice, mirroring the official provider SDKs.

use std::time::Duration;

/// Retry policy for transient provider failures.
///
/// A request is retried when the response status is 408, 409, 429, or any
/// 5xx, or when the transport fails to connect / times out before a response
/// arrives. Anything else — including every other 4xx — fails immediately.
///
/// The delay before attempt `n` (0-based) is
/// `min(initial_backoff * 2^n, max_backoff)` scaled by a random factor in
/// `[0.5, 1.0)` (full jitter). A `Retry-After` header on the response
/// overrides the computed delay when it parses as a number of seconds no
/// greater than [`RetryConfig::max_retry_after`].
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries after the initial attempt (default 2; the
    /// request is sent at most `max_retries + 1` times).
    pub max_retries: u32,
    /// Backoff before the first retry (default 500ms).
    pub initial_backoff: Duration,
    /// Upper bound on the computed backoff (default 8s).
    pub max_backoff: Duration,
    /// Largest `Retry-After` value that is honoured (default 60s). Larger
    /// values fall back to the computed backoff.
    pub max_retry_after: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(8),
            max_retry_after: Duration::from_secs(60),
        }
    }
}

impl RetryConfig {
    /// A policy that never retries — every failure surfaces immediately.
    pub fn disabled() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }

    /// Whether an HTTP status code is worth retrying.
    pub fn is_retryable_status(status: u16) -> bool {
        matches!(status, 408 | 409 | 429) || status >= 500
    }

    /// Delay before retry attempt `attempt` (0-based), honouring
    /// `retry_after` when present and within bounds.
    pub fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(ra) = retry_after {
            if ra <= self.max_retry_after {
                return ra;
            }
        }
        let exp = self
            .initial_backoff
            .saturating_mul(2u32.saturating_pow(attempt))
            .min(self.max_backoff);
        // Full jitter in [0.5, 1.0) of the computed delay. `RandomState` is
        // randomly seeded per process, which is plenty for backoff spread —
        // and keeps `rand` out of the always-on dependency set.
        let r = {
            use std::hash::{BuildHasher, Hasher};
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u32(attempt);
            (h.finish() % 1000) as f64 / 1000.0
        };
        exp.mul_f64(r.mul_add(0.5, 0.5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_statuses() {
        for s in [408u16, 409, 429, 500, 502, 503, 529] {
            assert!(RetryConfig::is_retryable_status(s), "{s} should retry");
        }
        for s in [200u16, 400, 401, 403, 404, 413] {
            assert!(!RetryConfig::is_retryable_status(s), "{s} should not");
        }
    }

    #[test]
    fn delay_grows_and_caps() {
        let cfg = RetryConfig::default();
        let d0 = cfg.delay(0, None);
        assert!(d0 >= Duration::from_millis(250) && d0 < Duration::from_millis(500));
        let d10 = cfg.delay(10, None);
        assert!(d10 <= cfg.max_backoff);
    }

    #[test]
    fn retry_after_honoured_within_bounds() {
        let cfg = RetryConfig::default();
        assert_eq!(
            cfg.delay(0, Some(Duration::from_secs(3))),
            Duration::from_secs(3)
        );
        // Too-large Retry-After falls back to computed backoff.
        assert!(cfg.delay(0, Some(Duration::from_secs(600))) < Duration::from_secs(1));
    }

    #[test]
    fn disabled_never_retries() {
        assert_eq!(RetryConfig::disabled().max_retries, 0);
    }
}
