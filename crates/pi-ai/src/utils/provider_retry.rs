//! HTTP request-layer retry for provider calls (match TS
//! `packages/ai/src/utils/provider-retry.ts` — `retryProviderRequest`).
//!
//! The TS OpenAI/Anthropic SDKs' built-in retry timers ignore the request
//! AbortSignal, so TS invokes the SDK with `maxRetries: 0` and wraps the
//! request with this helper. Rust has no SDK retry at all, so this is the
//! single place where transient HTTP errors (network, 408/409/429, 5xx) are
//! retried with exponential backoff, honoring `retry-after` headers and the
//! abort signal.

use std::time::Duration;

use tokio::sync::watch;

const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// A provider HTTP error carrying the status code (if any) so the retry
/// policy can decide whether the error is transient. `status: None` means a
/// transport/network error (no HTTP response) — always retryable.
#[derive(Debug)]
pub struct ProviderHttpError {
    pub status: Option<u16>,
    pub message: String,
    /// Raw `retry-after-ms` / `retry-after` header values (pre-parsed string).
    pub retry_after_ms: Option<f64>,
    pub retry_after: Option<String>,
    /// `x-should-retry` header value (match TS: true forces retry, false forbids).
    pub should_retry: Option<String>,
    /// Original provider-specific error (e.g. `PiMessagesResponseError`) so the
    /// caller can downcast for structured diagnostics after the retry loop.
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ProviderHttpError {
    pub fn new(status: Option<u16>, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            retry_after_ms: None,
            retry_after: None,
            should_retry: None,
            source: None,
        }
    }
}

/// Retry options (match TS `ProviderRetryOptions`).
#[derive(Debug, Clone, Default)]
pub struct RetryProviderOptions {
    /// Max retries (0 = no retries). The initial call never counts as a retry.
    pub max_retries: Option<u32>,
    /// Max server-requested delay in ms (default 60s). Set to 0 to disable.
    pub max_retry_delay_ms: Option<u64>,
    /// Abort signal; retries and backoff sleeps are interrupted when it flips.
    pub signal: Option<watch::Receiver<bool>>,
}

/// Whether the error should be retried (match TS `isRetryableProviderError`):
/// `x-should-retry: true` forces retry, `false` forbids; otherwise status
/// 408/409/429 or >=500, or no status (network error).
pub fn is_retryable_provider_error(err: &ProviderHttpError) -> bool {
    if let Some(ref should) = err.should_retry {
        match should.as_str() {
            "true" => return true,
            "false" => return false,
            _ => {}
        }
    }
    match err.status {
        None => true,
        Some(408) | Some(409) | Some(429) => true,
        Some(s) => s >= 500,
    }
}

/// Server-requested delay validation (match TS `validateServerRetryDelayMs`):
/// delays above `maxRetryDelayMs` fail immediately (60s default; 0 disables).
fn validate_server_retry_delay_ms(delay_ms: f64, max_retry_delay_ms: Option<u64>) -> Result<u64, String> {
    let max_delay_ms = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    if max_delay_ms > 0 && delay_ms as u64 > max_delay_ms {
        return Err(format!(
            "Server requested {}s retry delay (max: {}s)",
            (delay_ms / 1000.0).ceil(),
            (max_delay_ms as f64 / 1000.0).ceil()
        ));
    }
    Ok(delay_ms as u64)
}

/// Compute the delay before a retry (match TS `getRetryDelayMs`): prefers
/// `retry-after-ms`, then `retry-after`, else exponential backoff with jitter.
pub fn get_retry_delay_ms(
    err: &ProviderHttpError,
    retry_index: u32,
    max_retry_delay_ms: Option<u64>,
) -> Result<u64, String> {
    if let Some(ms) = err.retry_after_ms {
        if !ms.is_nan() {
            return validate_server_retry_delay_ms(ms, max_retry_delay_ms);
        }
    }
    if let Some(ref retry_after) = err.retry_after {
        let seconds: f64 = retry_after.trim().parse().unwrap_or(f64::NAN);
        let delay_ms = if seconds.is_nan() {
            // HTTP-date format: compute remaining time.
            let _ = retry_after;
            return Err("Unsupported retry-after date format".to_string());
        } else {
            seconds * 1000.0
        };
        return validate_server_retry_delay_ms(delay_ms, max_retry_delay_ms);
    }
    let exponential_delay = (0.5 * 2f64.powi(retry_index as i32)).min(8.0) * 1000.0;
    Ok((exponential_delay * (1.0 - rand_jitter())).max(0.0) as u64)
}

/// Uniform jitter in [0, 0.25) (match TS `1 - Math.random() * 0.25`).
fn rand_jitter() -> f64 {
    // Simple deterministic-ish jitter without pulling in rand: use a cheap
    // hash of a counter. (rand crate is not a workspace dep of pi-ai.)
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // xorshift64 — fast, good enough for jitter.
    let mut x = n.wrapping_add(0x9E3779B97F4A7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D049BB133111EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64 * 0.25
}

/// Abortable sleep (match TS `abortableSleep`): rejects/returns early when the
/// signal flips to true.
async fn abortable_sleep(ms: u64, signal: Option<watch::Receiver<bool>>) {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
        _ = async {
            if let Some(mut rx) = signal {
                loop {
                    if *rx.borrow() {
                        break;
                    }
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            } else {
                std::future::pending::<()>().await;
            }
        } => {}
    }
}

/// Retry a provider request (match TS `retryProviderRequest`). The closure is
/// invoked fresh on each attempt; a successful `Ok` is returned immediately.
pub async fn retry_provider_request<F, Fut, T>(
    request: F,
    options: RetryProviderOptions,
) -> Result<T, ProviderHttpError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ProviderHttpError>>,
{
    let max_retries = options.max_retries.unwrap_or(0);
    let mut retries_remaining = max_retries;
    loop {
        match request().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                // Abort check: if the signal flipped, fail fast with the
                // original error (match TS `if (options.signal?.aborted) throw`).
                if let Some(ref rx) = options.signal {
                    if *rx.borrow() {
                        return Err(err);
                    }
                }
                if retries_remaining == 0 || !is_retryable_provider_error(&err) {
                    return Err(err);
                }
                let retry_index = max_retries - retries_remaining;
                retries_remaining -= 1;
                match get_retry_delay_ms(&err, retry_index, options.max_retry_delay_ms) {
                    Ok(delay_ms) => abortable_sleep(delay_ms, options.signal.clone()).await,
                    Err(e) => {
                        // Server requested too long a delay — fail immediately
                        // (match TS validateServerRetryDelayMs throw).
                        let mut fatal = err;
                        fatal.message = format!("{e}. {}", fatal.message);
                        return Err(fatal);
                    }
                }
            }
        }
    }
}

/// Shared convenience wrapper for provider HTTP requests: sends `request` via
/// `http_client`, retrying transient failures per [`retry_provider_request`],
/// and returns the response on success. Non-2xx responses are surfaced as
/// errors (with retry headers extracted for the retry policy).
///
/// Falls back to a single attempt when the request body cannot be cloned.
pub async fn send_with_retry(
    request: reqwest::Request,
    http_client: &reqwest::Client,
    signal: Option<watch::Receiver<bool>>,
    max_retries: Option<u32>,
    max_retry_delay_ms: Option<u64>,
    error_prefix: &str,
) -> Result<reqwest::Response, String> {
    let http_client = http_client.clone();
    let max_retries = max_retries.unwrap_or(0);
    match request.try_clone() {
        Some(request) => {
            let request_ref = &request;
            retry_provider_request(
                || {
                    let http_client = http_client.clone();
                    async move {
                        let req = request_ref.try_clone().ok_or_else(|| {
                            ProviderHttpError::new(None, "request body not cloneable")
                        })?;
                        let response = http_client
                            .execute(req)
                            .await
                            .map_err(|e| ProviderHttpError::new(e.status().map(|s| s.as_u16()), e.to_string()))?;
                        let status = response.status();
                        let headers = response.headers().clone();
                        if !status.is_success() {
                            let text = response.text().await.unwrap_or_default();
                            let mut err = ProviderHttpError::new(
                                Some(status.as_u16()),
                                format!("{error_prefix} {status}: {text}"),
                            );
                            err.retry_after_ms = headers
                                .get("retry-after-ms")
                                .and_then(|v| v.to_str().ok())
                                .and_then(|v| v.trim().parse::<f64>().ok());
                            err.retry_after = headers
                                .get("retry-after")
                                .and_then(|v| v.to_str().ok())
                                .map(|s| s.to_string());
                            err.should_retry = headers
                                .get("x-should-retry")
                                .and_then(|v| v.to_str().ok())
                                .map(|s| s.to_string());
                            return Err(err);
                        }
                        Ok(response)
                    }
                },
                RetryProviderOptions {
                    max_retries: Some(max_retries),
                    max_retry_delay_ms,
                    signal,
                },
            )
            .await
            .map_err(|e| e.message)
        }
        None => {
            let response = http_client
                .execute(request)
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status();
            if !status.is_success() {
                let text = response.text().await.unwrap_or_default();
                return Err(format!("{error_prefix} {status}: {text}"));
            }
            Ok(response)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable_provider_error(&ProviderHttpError::new(Some(408), "x")));
        assert!(is_retryable_provider_error(&ProviderHttpError::new(Some(409), "x")));
        assert!(is_retryable_provider_error(&ProviderHttpError::new(Some(429), "x")));
        assert!(is_retryable_provider_error(&ProviderHttpError::new(Some(500), "x")));
        assert!(is_retryable_provider_error(&ProviderHttpError::new(Some(503), "x")));
        assert!(is_retryable_provider_error(&ProviderHttpError::new(None, "network")));
        assert!(!is_retryable_provider_error(&ProviderHttpError::new(Some(400), "x")));
        assert!(!is_retryable_provider_error(&ProviderHttpError::new(Some(401), "x")));
        assert!(!is_retryable_provider_error(&ProviderHttpError::new(Some(404), "x")));
    }

    #[test]
    fn should_retry_header_overrides_status() {
        let mut err = ProviderHttpError::new(Some(400), "x");
        err.should_retry = Some("true".to_string());
        assert!(is_retryable_provider_error(&err));
        err.should_retry = Some("false".to_string());
        assert!(!is_retryable_provider_error(&err));
    }

    #[test]
    fn retry_after_ms_preferred_over_backoff() {
        let mut err = ProviderHttpError::new(Some(429), "x");
        err.retry_after_ms = Some(250.0);
        assert_eq!(get_retry_delay_ms(&err, 0, None).unwrap(), 250);
    }

    #[test]
    fn server_delay_over_max_fails() {
        let mut err = ProviderHttpError::new(Some(429), "x");
        err.retry_after_ms = Some(120_000.0);
        assert!(get_retry_delay_ms(&err, 0, Some(60_000)).is_err());
    }

    #[tokio::test]
    async fn retries_transient_errors_then_succeeds() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_provider_request(
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    let n = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n < 2 {
                        Err::<&str, _>(ProviderHttpError::new(Some(503), "overloaded"))
                    } else {
                        Ok::<&str, ProviderHttpError>("ok")
                    }
                }
            },
            RetryProviderOptions {
                max_retries: Some(3),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn no_retry_on_non_transient() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_provider_request(
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err::<(), _>(ProviderHttpError::new(Some(400), "bad request"))
                }
            },
            RetryProviderOptions {
                max_retries: Some(3),
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn abort_interrupts_retry_backoff() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let fut = retry_provider_request(
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err::<(), _>(ProviderHttpError::new(Some(503), "overloaded"))
                }
            },
            RetryProviderOptions {
                max_retries: Some(10),
                signal: Some(rx),
                ..Default::default()
            },
        );
        tokio::pin!(fut);
        // Let the first attempt fail and enter backoff.
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            _ = &mut fut => panic!("should still be in backoff"),
        }
        tx.send(true).unwrap();
        let err = fut.await.unwrap_err();
        assert_eq!(err.status, Some(503));
    }
}
