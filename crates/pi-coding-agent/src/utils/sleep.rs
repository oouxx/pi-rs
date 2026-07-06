//! Sleep helper that can be aborted.
//!
//! Mirrors packages/coding-agent/src/utils/sleep.ts

use tokio::time::{sleep, Duration};

/// Sleep for `ms` milliseconds, respecting an abort signal.
pub async fn sleep_ms(ms: u64) {
    sleep(Duration::from_millis(ms)).await;
}

/// Sleep for `ms` milliseconds, returning early if `signal` is triggered.
/// Returns `true` if slept the full duration, `false` if aborted.
pub async fn sleep_ms_with_signal(
    ms: u64,
    signal: Option<tokio::sync::watch::Receiver<bool>>,
) -> bool {
    let sleep_fut = sleep(Duration::from_millis(ms));

    if let Some(mut signal) = signal {
        tokio::select! {
            _ = sleep_fut => true,
            _ = signal.changed() => false,
        }
    } else {
        sleep_fut.await;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sleep_ms() {
        let start = std::time::Instant::now();
        sleep_ms(10).await;
        assert!(start.elapsed().as_millis() >= 8);
    }
}
