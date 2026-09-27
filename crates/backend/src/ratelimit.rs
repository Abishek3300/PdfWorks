//! Per-client Rate_Limiter backed by counters (Task 14.1, Req 42.1-42.3).
//!
//! The Rate_Limiter caps Job submissions and uploads per client within a
//! sliding window (Req 42.1, 42.2) and returns a rate-limit signal when a
//! client exceeds a limit (Req 42.3 → HTTP 429 at the gateway). Counters live
//! in Redis in production; the [`CounterStore`] trait lets tests use an
//! in-memory counter with identical semantics.
//!
//! A fixed-window counter is used: each `(client, category, window)` triple has
//! a counter that is incremented per request and expires at the end of the
//! window. This is the classic Redis `INCR` + `EXPIRE` pattern.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use crate::config::RateLimitConfig;

/// Which action is being rate-limited (Req 42.1 vs 42.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitedAction {
    /// A Job submission (Req 42.1).
    JobSubmission,
    /// An upload (Req 42.2).
    Upload,
}

impl LimitedAction {
    fn key_part(self) -> &'static str {
        match self {
            LimitedAction::JobSubmission => "job",
            LimitedAction::Upload => "upload",
        }
    }
}

/// A monotonic counter store with per-key expiry (the Redis `INCR`/`EXPIRE`
/// contract).
#[async_trait]
pub trait CounterStore: Send + Sync {
    /// Increment `key` and return its new value; set `ttl` on first creation.
    async fn incr(&self, key: &str, ttl: Duration) -> Result<u64, CounterError>;
}

/// Errors from the counter store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CounterError {
    /// The backing store failed.
    Backend(String),
}

/// An in-memory fixed-window counter for tests. The window is derived from a
/// caller-supplied clock so tests are deterministic.
pub struct InMemoryCounterStore {
    counts: Mutex<HashMap<String, u64>>,
}

impl Default for InMemoryCounterStore {
    fn default() -> Self {
        Self {
            counts: Mutex::new(HashMap::new()),
        }
    }
}

impl InMemoryCounterStore {
    /// Create an empty counter store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CounterStore for InMemoryCounterStore {
    async fn incr(&self, key: &str, _ttl: Duration) -> Result<u64, CounterError> {
        let mut counts = self
            .counts
            .lock()
            .map_err(|_| CounterError::Backend("lock".to_string()))?;
        let n = counts.entry(key.to_string()).or_insert(0);
        *n += 1;
        Ok(*n)
    }
}

/// The Rate_Limiter over a [`CounterStore`].
pub struct RateLimiter<C: CounterStore> {
    store: C,
    config: RateLimitConfig,
}

impl<C: CounterStore> RateLimiter<C> {
    /// Build a limiter with the given counter store + config.
    #[must_use]
    pub fn new(store: C, config: RateLimitConfig) -> Self {
        Self { store, config }
    }

    /// Compute the fixed-window bucket key for a client + action at `now_ms`.
    fn window_key(&self, client_ref: &str, action: LimitedAction, now_ms: u64) -> String {
        let window_ms = self.config.window.as_millis().max(1) as u64;
        let bucket = now_ms / window_ms;
        format!("rl:{}:{}:{}", action.key_part(), client_ref, bucket)
    }

    /// Record a request and return whether it is *allowed* (within the limit).
    /// A `false` result means the client exceeded its limit and the gateway
    /// should return 429 (Req 42.3).
    ///
    /// # Errors
    ///
    /// Propagates counter-store errors.
    pub async fn check(
        &self,
        client_ref: &str,
        action: LimitedAction,
        now_ms: u64,
    ) -> Result<bool, CounterError> {
        let key = self.window_key(client_ref, action, now_ms);
        let count = self.store.incr(&key, self.config.window).await?;
        let limit = match action {
            LimitedAction::JobSubmission => self.config.max_jobs_per_window,
            LimitedAction::Upload => self.config.max_uploads_per_window,
        };
        Ok(count <= u64::from(limit))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn limiter() -> RateLimiter<InMemoryCounterStore> {
        RateLimiter::new(
            InMemoryCounterStore::new(),
            RateLimitConfig {
                window: Duration::from_secs(60),
                max_jobs_per_window: 3,
                max_uploads_per_window: 5,
            },
        )
    }

    #[tokio::test]
    async fn allows_up_to_limit_then_blocks() {
        let l = limiter();
        for _ in 0..3 {
            assert!(l.check("cA", LimitedAction::JobSubmission, 0).await.unwrap());
        }
        // 4th submission in the same window is blocked.
        assert!(!l.check("cA", LimitedAction::JobSubmission, 0).await.unwrap());
    }

    #[tokio::test]
    async fn separate_clients_have_separate_budgets() {
        let l = limiter();
        for _ in 0..3 {
            assert!(l.check("cA", LimitedAction::JobSubmission, 0).await.unwrap());
        }
        // A different client still has its full budget.
        assert!(l.check("cB", LimitedAction::JobSubmission, 0).await.unwrap());
    }

    #[tokio::test]
    async fn new_window_resets_budget() {
        let l = limiter();
        for _ in 0..3 {
            assert!(l.check("cA", LimitedAction::JobSubmission, 0).await.unwrap());
        }
        assert!(!l.check("cA", LimitedAction::JobSubmission, 0).await.unwrap());
        // 61s later is a new fixed window -> budget resets.
        assert!(l.check("cA", LimitedAction::JobSubmission, 61_000).await.unwrap());
    }

    #[tokio::test]
    async fn uploads_and_jobs_are_independent() {
        let l = limiter();
        for _ in 0..3 {
            assert!(l.check("cA", LimitedAction::JobSubmission, 0).await.unwrap());
        }
        // Job budget exhausted, but uploads still allowed.
        assert!(l.check("cA", LimitedAction::Upload, 0).await.unwrap());
    }
}
