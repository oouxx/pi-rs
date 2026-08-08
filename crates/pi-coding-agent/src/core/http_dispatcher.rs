pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

/// Apply the global `httpProxy` setting to the process environment so that
/// reqwest (which reads `HTTP_PROXY`/`HTTPS_PROXY` when no explicit proxy is
/// configured) routes requests through it. Matches TS `applyHttpProxySettings`:
/// only set the env vars when they are not already present (`??=` semantics).
pub fn apply_http_proxy_settings(http_proxy: Option<&str>) {
    let Some(proxy) = http_proxy.map(str::trim).filter(|p| !p.is_empty()) else {
        return;
    };
    if std::env::var("HTTP_PROXY").is_err() {
        std::env::set_var("HTTP_PROXY", proxy);
    }
    if std::env::var("HTTPS_PROXY").is_err() {
        std::env::set_var("HTTPS_PROXY", proxy);
    }
}

pub const HTTP_IDLE_TIMEOUT_CHOICES: &[(u64, &str)] = &[
    (30_000, "30 sec"),
    (60_000, "1 min"),
    (120_000, "2 min"),
    (300_000, "5 min"),
    (0, "disabled"),
];

pub fn parse_http_idle_timeout_ms(value: &str) -> Option<u64> {
    let trimmed = value.trim();

    if trimmed.eq_ignore_ascii_case("disabled") {
        return Some(0);
    }

    if trimmed.is_empty() {
        return None;
    }

    let num: f64 = trimmed.parse().ok()?;
    if !num.is_finite() || num < 0.0 {
        return None;
    }
    Some(num.floor() as u64)
}

pub fn format_http_idle_timeout_ms(timeout_ms: u64) -> String {
    for &(ms, label) in HTTP_IDLE_TIMEOUT_CHOICES {
        if ms == timeout_ms {
            return label.to_string();
        }
    }
    format!("{} sec", timeout_ms / 1000)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_apply_http_proxy_sets_env_when_unset() {
        let _guard = EnvGuard::new();
        std::env::remove_var("HTTP_PROXY");
        std::env::remove_var("HTTPS_PROXY");
        apply_http_proxy_settings(Some("http://proxy.local:8080"));
        assert_eq!(std::env::var("HTTP_PROXY").unwrap(), "http://proxy.local:8080");
        assert_eq!(std::env::var("HTTPS_PROXY").unwrap(), "http://proxy.local:8080");
    }

    #[test]
    fn test_apply_http_proxy_keeps_existing_env() {
        let _guard = EnvGuard::new();
        std::env::set_var("HTTP_PROXY", "http://existing:1");
        std::env::set_var("HTTPS_PROXY", "http://existing:2");
        apply_http_proxy_settings(Some("http://proxy.local:8080"));
        assert_eq!(std::env::var("HTTP_PROXY").unwrap(), "http://existing:1");
        assert_eq!(std::env::var("HTTPS_PROXY").unwrap(), "http://existing:2");
    }

    #[test]
    fn test_apply_http_proxy_ignores_empty() {
        let _guard = EnvGuard::new();
        std::env::remove_var("HTTP_PROXY");
        std::env::remove_var("HTTPS_PROXY");
        apply_http_proxy_settings(Some("   "));
        apply_http_proxy_settings(None);
        assert!(std::env::var("HTTP_PROXY").is_err());
        assert!(std::env::var("HTTPS_PROXY").is_err());
    }

    /// Serialize env-mutating tests (process-global env is shared across the
    /// parallel test threads) and restore the vars afterwards.
    struct EnvGuard {
        http_was_unset: bool,
        https_was_unset: bool,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new() -> Self {
            static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            EnvGuard {
                http_was_unset: std::env::var("HTTP_PROXY").is_err(),
                https_was_unset: std::env::var("HTTPS_PROXY").is_err(),
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if self.http_was_unset {
                std::env::remove_var("HTTP_PROXY");
            }
            if self.https_was_unset {
                std::env::remove_var("HTTPS_PROXY");
            }
        }
    }

    #[test]
    fn test_parse_disabled() {
        assert_eq!(parse_http_idle_timeout_ms("disabled"), Some(0));
    }

    #[test]
    fn test_parse_number_string() {
        assert_eq!(parse_http_idle_timeout_ms("120000"), Some(120000));
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(parse_http_idle_timeout_ms(""), None);
    }

    #[test]
    fn test_parse_invalid() {
        assert_eq!(parse_http_idle_timeout_ms("not-a-number"), None);
    }

    #[test]
    fn test_format_known_choice() {
        assert_eq!(format_http_idle_timeout_ms(30000), "30 sec");
    }

    #[test]
    fn test_format_custom_value() {
        assert_eq!(format_http_idle_timeout_ms(45000), "45 sec");
    }

    #[test]
    fn test_format_disabled() {
        assert_eq!(format_http_idle_timeout_ms(0), "disabled");
    }
}
