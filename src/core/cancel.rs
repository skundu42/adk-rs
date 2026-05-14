//! Per-invocation cancellation flag.
//!
//! A [`CancellationToken`] is a cheap, cloneable handle wrapping a single
//! [`std::sync::atomic::AtomicBool`]. The [`crate::runner::Runner`] hands
//! one to every invocation via [`crate::core::InvocationContext`]; agents
//! check it at safe points (between LLM calls, between sub-agents) and
//! short-circuit when it flips. Callers flip it via
//! [`crate::runner::Runner::cancel`].
//!
//! The implementation deliberately avoids `tokio_util::sync::CancellationToken`:
//! adk-rs already pulls in `tokio` everywhere; agents observe the token
//! synchronously between awaits, so an `AtomicBool` is sufficient and
//! costs no extra dependency.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cooperative cancellation flag. Cheap to [`Clone`]; all clones share
/// the same underlying state.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Fresh token in the "not cancelled" state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Flip the token to "cancelled". Idempotent — repeated calls are
    /// no-ops.
    pub fn cancel(&self) {
        self.inner.store(true, Ordering::Release);
    }

    /// True once [`Self::cancel`] has been called on this token (or any
    /// clone).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_token_is_not_cancelled() {
        assert!(!CancellationToken::new().is_cancelled());
    }

    #[test]
    fn cancel_flips_for_all_clones() {
        let a = CancellationToken::new();
        let b = a.clone();
        assert!(!a.is_cancelled());
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(a.is_cancelled());
        assert!(b.is_cancelled());
    }

    #[test]
    fn cancel_is_idempotent() {
        let t = CancellationToken::new();
        t.cancel();
        t.cancel();
        assert!(t.is_cancelled());
    }
}
