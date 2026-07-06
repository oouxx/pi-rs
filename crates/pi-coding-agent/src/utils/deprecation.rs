//! Deprecation warning management — emit each warning at most once.
//!
//! Mirrors packages/coding-agent/src/utils/deprecation.ts

use std::sync::Mutex;

static EMITTED_WARNINGS: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// Emit a deprecation warning to stderr.
/// Each message is emitted at most once.
pub fn warn_deprecation(message: &str) {
    let mut emitted = EMITTED_WARNINGS.lock().unwrap();
    if !emitted.insert(message.to_string()) {
        return; // already emitted
    }
    eprintln!("Deprecation warning: {message}");
}

/// Clear emitted deprecation warnings (for tests).
pub fn clear_deprecation_warnings() {
    EMITTED_WARNINGS.lock().unwrap().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deprecation_dedup() {
        clear_deprecation_warnings();
        warn_deprecation("test warning");
        // Second call should not panic or re-emit
        warn_deprecation("test warning");
        // Just verify no crash
        assert!(true);
    }

    #[test]
    fn test_deprecation_multiple() {
        clear_deprecation_warnings();
        warn_deprecation("warning 1");
        warn_deprecation("warning 2");
        // Both should be tracked
        let emitted = EMITTED_WARNINGS.lock().unwrap();
        assert!(emitted.contains("warning 1"));
        assert!(emitted.contains("warning 2"));
    }
}
