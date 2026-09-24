//! HTTP idle-timeout helpers (match TS `httpIdleTimeoutMs` → undici
//! `bodyTimeout` / `headersTimeout` in `packages/coding-agent/src/core/http-dispatcher.ts`).
//!
//! TS installs an undici global dispatcher with `headersTimeout` and
//! `bodyTimeout` set to `httpIdleTimeoutMs` (default 300s). The timeout is an
//! *idle* timeout: it fires only when no response data arrives for that long,
//! so a long-but-active stream is never cut off. When it fires, the fetch body
//! read rejects with a timeout error, which the agent-level auto-retry
//! classifies as retryable.
//!
//! The Rust providers previously accepted `StreamOptions.timeout_ms` but never
//! applied it, so a stalled upstream hung forever (observed: a 4.5-minute hang
//! that the user manually interrupted). These helpers wire the setting into the
//! SSE read loops.

use std::time::Duration;

/// Largest `timeout_ms` value the SDKs treat as "disabled" (TS uses
/// `2147483647` when `httpIdleTimeoutMs === 0`).
pub const DISABLED_TIMEOUT_MS: u64 = 2_147_483_647;

/// Resolve `StreamOptions.timeout_ms` into an optional idle timeout.
/// `0` and the "disabled" sentinel yield `None`.
pub fn resolve_idle_timeout(timeout_ms: Option<u64>) -> Option<Duration> {
    timeout_ms
        .filter(|ms| *ms > 0 && *ms < DISABLED_TIMEOUT_MS)
        .map(Duration::from_millis)
}

/// Await `fut`, failing with `Err(())` if it does not complete within `idle`.
/// When `idle` is `None` the future is awaited without a timeout.
///
/// Cancel-safe: the inner future is only dropped if the timeout fires, and
/// callers typically select this against an abort future.
pub async fn with_idle_timeout<F: std::future::Future>(
    fut: F,
    idle: Option<Duration>,
) -> Result<F::Output, ()> {
    match idle {
        Some(idle) => tokio::time::timeout(idle, fut).await.map_err(|_| ()),
        None => Ok(fut.await),
    }
}

/// Error message for an idle timeout. Contains `timeout` so the agent-level
/// retry classifier treats it as a transient error (matching TS, where the
/// undici timeout rejects the body read and the turn is retried).
pub fn idle_timeout_message(idle: Duration) -> String {
    format!(
        "Request timed out: no response data received for {}s",
        idle.as_secs()
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn resolve_idle_timeout_filters_disabled_values() {
        assert_eq!(resolve_idle_timeout(None), None);
        assert_eq!(resolve_idle_timeout(Some(0)), None);
        assert_eq!(resolve_idle_timeout(Some(DISABLED_TIMEOUT_MS)), None);
        assert_eq!(
            resolve_idle_timeout(Some(300_000)),
            Some(Duration::from_millis(300_000))
        );
    }

    #[tokio::test]
    async fn with_idle_timeout_times_out() {
        let result = with_idle_timeout(
            std::future::pending::<()>(),
            Some(Duration::from_millis(10)),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn with_idle_timeout_passes_through_when_disabled() {
        let result = with_idle_timeout(async { 42 }, None).await;
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn idle_timeout_message_is_retryable() {
        // The retry classifier matches `timed? out` / `timeout`; keep the
        // message aligned so an idle timeout is auto-retried like TS.
        let msg = idle_timeout_message(Duration::from_secs(300));
        assert!(msg.contains("timed out"), "message was: {msg}");
    }
}
