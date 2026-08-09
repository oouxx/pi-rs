//! Bounded retry for assistant-producing calls (match TS
//! `packages/ai/src/utils/retry.ts`).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::types::{AssistantMessage, StopReason};

/// Retry policy: bounded attempts with exponential backoff
/// (`baseDelayMs * 2^(attempt-1)`). Matches `settings.retry`
/// (`enabled`, `maxRetries`, `baseDelayMs`) in coding-agent.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub enabled: bool,
    /// Max retry attempts (0 = no retries). The initial call never counts as a retry.
    pub max_retries: u32,
    /// Base delay in ms. Per-attempt delay is `baseDelayMs * 2^(attempt-1)` before jitter.
    pub base_delay_ms: u64,
}

/// Optional callbacks emitted by [`retry_assistant_call`] around each retry.
#[derive(Default)]
pub struct RetryCallbacks {
    /// Emitted before the backoff sleep of each retry attempt (1-indexed).
    pub on_retry_scheduled: Option<RetryScheduledFn>,
    /// Emitted after the backoff sleep, immediately before the retried call starts.
    pub on_retry_attempt_start: Option<RetryAttemptStartFn>,
    /// Emitted once when the loop ends: success if a later call completed normally.
    pub on_retry_finished: Option<RetryFinishedFn>,
}

/// Callback type for [`RetryCallbacks::on_retry_scheduled`].
pub type RetryScheduledFn = Arc<dyn Fn(u32, u32, u64, String) + Send + Sync>;
/// Callback type for [`RetryCallbacks::on_retry_attempt_start`].
pub type RetryAttemptStartFn = Arc<dyn Fn() + Send + Sync>;
/// Callback type for [`RetryCallbacks::on_retry_finished`].
pub type RetryFinishedFn = Arc<dyn Fn(bool, u32, Option<String>) + Send + Sync>;

/// Non-retryable patterns (quota/billing/limit errors) — match TS
/// `NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN`.
const NON_RETRYABLE_PATTERNS: &[&str] = &[
    "GoUsageLimitError",
    "FreeUsageLimitError",
    "Monthly usage limit reached",
    "available balance",
    "insufficient_quota",
    "out of budget",
    "quota exceeded",
    "billing",
];

/// Retryable patterns (transient provider/transport errors) — match TS
/// `RETRYABLE_PROVIDER_ERROR_PATTERN`.
const RETRYABLE_PATTERNS: &[&str] = &[
    "overloaded",
    "rate limit",
    "too many requests",
    "429",
    "500",
    "502",
    "503",
    "504",
    "524",
    "service unavailable",
    "server error",
    "internal error",
    "provider returned error",
    "network error",
    "connection error",
    "connection refused",
    "connection lost",
    "other side closed",
    "fetch failed",
    "upstream connect",
    "reset before headers",
    "socket hang up",
    "socket connection was closed",
    "timed out",
    "timeout",
    "terminated",
    "websocket closed",
    "websocket error",
    "ended without",
    "stream ended before message_stop",
    "stream ended before a terminal response event",
    "http2 request did not get a response",
    "retry delay",
    "you can retry your request",
    "try your request again",
    "please retry your request",
    "ResourceExhausted",
];

/// Classifies whether a failed assistant message looks like a transient provider
/// or transport error (match TS `isRetryableAssistantError`).
pub fn is_retryable_assistant_error(message: &AssistantMessage) -> bool {
    if message.stop_reason != StopReason::Error {
        return false;
    }
    let Some(error_message) = message.error_message.as_deref() else {
        return false;
    };
    let lower = error_message.to_lowercase();
    if NON_RETRYABLE_PATTERNS.iter().any(|p| lower.contains(p)) {
        return false;
    }
    RETRYABLE_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Run a single assistant-producing call with bounded retry on transient errors
/// (match TS `retryAssistantCall`).
///
/// Behavior:
/// - A successful response is returned immediately. Aborts are terminal and never
///   retried, but reported as unsuccessful if they happen after a retry was scheduled.
///   Aborts during the backoff sleep are normalized to an aborted `AssistantMessage`
///   too, so callers do not need to care when cancellation happened.
/// - A non-retryable error (per [`is_retryable_assistant_error`], including quota/
///   billing exhaustion) is returned immediately so deterministic errors fail fast.
/// - Otherwise retries up to `max_retries` times with exponential backoff, emitting
///   `on_retry_scheduled` before each sleep, `on_retry_attempt_start` after each sleep
///   before the retried call starts, and `on_retry_finished` once at the end.
///
/// When `policy` is `None` or disabled, the first response is returned unchanged.
pub async fn retry_assistant_call<F, Fut>(
    produce: F,
    policy: Option<RetryPolicy>,
    signal: Option<&watch::Receiver<bool>>,
    callbacks: Option<&RetryCallbacks>,
) -> AssistantMessage
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = AssistantMessage>,
{
    let max_attempts = match policy {
        Some(p) if p.enabled => p.max_retries,
        _ => 0,
    };

    let mut attempt: u32 = 0;
    let mut last_retry: Option<(u32, String)> = None;

    loop {
        let response = produce().await;

        // Abort: terminal but not successful. Never retry an aborted message.
        if response.stop_reason == StopReason::Aborted {
            if let Some((a, _)) = &last_retry {
                if let Some(cb) = callbacks.and_then(|c| c.on_retry_finished.as_ref()) {
                    cb(false, *a, None);
                }
            }
            return response;
        }

        // Success: non-error, non-abort responses return as-is.
        if response.stop_reason != StopReason::Error {
            if let Some((a, _)) = &last_retry {
                if let Some(cb) = callbacks.and_then(|c| c.on_retry_finished.as_ref()) {
                    cb(true, *a, None);
                }
            }
            return response;
        }

        // Non-retryable, or budget exhausted: return the final error message.
        if attempt >= max_attempts || !is_retryable_assistant_error(&response) {
            if let Some((a, _)) = &last_retry {
                if let Some(cb) = callbacks.and_then(|c| c.on_retry_finished.as_ref()) {
                    cb(false, *a, response.error_message.clone());
                }
            }
            return response;
        }

        attempt += 1;
        let error_message = response.error_message.clone().unwrap_or_else(|| "Unknown error".to_string());
        last_retry = Some((attempt, error_message.clone()));
        let base_delay = policy.map(|p| p.base_delay_ms).unwrap_or(0);
        let delay_ms = base_delay * 2u64.pow(attempt - 1);
        if let Some(cb) = callbacks.and_then(|c| c.on_retry_scheduled.as_ref()) {
            cb(attempt, max_attempts, delay_ms, error_message.clone());
        }

        // Normalize aborts during retry backoff to the same AssistantMessage shape
        // as provider stream aborts, so callers do not need to care when cancellation
        // happened.
        let aborted_during_sleep = sleep_abortable(Duration::from_millis(delay_ms), signal).await;
        if aborted_during_sleep {
            if let Some(cb) = callbacks.and_then(|c| c.on_retry_finished.as_ref()) {
                cb(false, attempt, Some(error_message));
            }
            return AssistantMessage {
                stop_reason: StopReason::Aborted,
                error_message: None,
                ..response
            };
        }
        if let Some(cb) = callbacks.and_then(|c| c.on_retry_attempt_start.as_ref()) {
            cb();
        }
    }
}

/// Sleep that can be aborted via the watch channel. Returns `true` if aborted.
async fn sleep_abortable(duration: Duration, signal: Option<&watch::Receiver<bool>>) -> bool {
    let Some(signal) = signal else {
        tokio::time::sleep(duration).await;
        return false;
    };
    if *signal.borrow() {
        return true;
    }
    let mut rx = signal.clone();
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        changed = rx.changed() => {
            let _ = changed;
            *rx.borrow()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_msg(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: crate::types::Usage::default(),
            stop_reason: StopReason::Error,
            error_message: Some(text.to_string()),
            raw_stop_reason: None,
            timestamp: 0,
        }
    }

    fn ok_msg() -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: crate::types::Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        }
    }

    #[test]
    fn test_is_retryable_assistant_error() {
        assert!(is_retryable_assistant_error(&error_msg("stream ended before a terminal response event")));
        assert!(is_retryable_assistant_error(&error_msg("Provider returned error: 503")));
        assert!(is_retryable_assistant_error(&error_msg("connection refused")));
        assert!(!is_retryable_assistant_error(&error_msg("insufficient_quota")));
        assert!(!is_retryable_assistant_error(&error_msg("Monthly usage limit reached")));
        assert!(!is_retryable_assistant_error(&ok_msg()));
    }

    #[tokio::test]
    async fn test_retry_assistant_call_success_first_try() {
        let result = retry_assistant_call(|| async { ok_msg() }, None, None, None).await;
        assert_eq!(result.stop_reason, StopReason::Stop);
    }

    #[tokio::test]
    async fn test_retry_assistant_call_retries_then_succeeds() {
        let calls = std::sync::atomic::AtomicU32::new(0);
        let result = retry_assistant_call(
            || {
                let calls = &calls;
                async move {
                    let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        error_msg("503 Service Unavailable")
                    } else {
                        ok_msg()
                    }
                }
            },
            Some(RetryPolicy {
                enabled: true,
                max_retries: 3,
                base_delay_ms: 1,
            }),
            None,
            None,
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Stop);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_assistant_call_exhausts_retries() {
        let calls = std::sync::atomic::AtomicU32::new(0);
        let result = retry_assistant_call(
            || {
                let calls = &calls;
                async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    error_msg("503 Service Unavailable")
                }
            },
            Some(RetryPolicy {
                enabled: true,
                max_retries: 2,
                base_delay_ms: 1,
            }),
            None,
            None,
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Error);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3); // initial + 2 retries
    }

    #[tokio::test]
    async fn test_retry_assistant_call_non_retryable_fails_fast() {
        let calls = std::sync::atomic::AtomicU32::new(0);
        let result = retry_assistant_call(
            || {
                let calls = &calls;
                async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    error_msg("insufficient_quota")
                }
            },
            Some(RetryPolicy {
                enabled: true,
                max_retries: 3,
                base_delay_ms: 1,
            }),
            None,
            None,
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Error);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_assistant_call_abort_during_backoff() {
        let (tx, rx) = watch::channel(false);
        let result = retry_assistant_call(
            || async { error_msg("503 Service Unavailable") },
            Some(RetryPolicy {
                enabled: true,
                max_retries: 3,
                base_delay_ms: 10_000,
            }),
            Some(&rx),
            None,
        );
        // abort after a short delay
        let abort = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(true);
        };
        let (result, _) = tokio::join!(result, abort);
        assert_eq!(result.stop_reason, StopReason::Aborted);
    }
}
