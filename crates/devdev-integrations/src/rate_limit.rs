//! Rate-limit tracking for GitHub API.

use std::sync::atomic::{AtomicU64, Ordering};

/// Tracks GitHub API rate-limit state.
pub struct RateLimitTracker {
    remaining: AtomicU64,
    reset_at: AtomicU64,
}

impl RateLimitTracker {
    pub fn new() -> Self {
        Self {
            remaining: AtomicU64::new(u64::MAX),
            reset_at: AtomicU64::new(0),
        }
    }

    /// Update from response headers.
    pub fn update(&self, remaining: u64, reset_at: u64) {
        self.remaining.store(remaining, Ordering::Relaxed);
        self.reset_at.store(reset_at, Ordering::Relaxed);

        if remaining < 10 {
            tracing::warn!(remaining, reset_at, "GitHub API rate limit low");
        }
    }

    /// Current remaining requests.
    pub fn remaining(&self) -> u64 {
        self.remaining.load(Ordering::Relaxed)
    }

    /// Unix timestamp when the limit resets.
    pub fn reset_at(&self) -> u64 {
        self.reset_at.load(Ordering::Relaxed)
    }
}

impl Default for RateLimitTracker {
    fn default() -> Self {
        Self::new()
    }
}
