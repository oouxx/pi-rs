/// Experimental feature flags for pi-coding-agent.
///
/// Mirrors packages/coding-agent/src/core/experimental.ts

/// Check whether experimental features are enabled.
///
/// Reads the `PI_EXPERIMENTAL` environment variable.
/// Returns `true` only when the value is exactly `"1"`.
pub fn are_experimental_features_enabled() -> bool {
    std::env::var("PI_EXPERIMENTAL").as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run tests sequentially in a single test function to avoid env var races.
    #[test]
    fn test_experimental_all() {
        // 1. Disabled by default
        std::env::remove_var("PI_EXPERIMENTAL");
        assert!(!are_experimental_features_enabled());

        // 2. Enabled when set to "1"
        std::env::set_var("PI_EXPERIMENTAL", "1");
        assert!(are_experimental_features_enabled());

        // 3. Not enabled for other values
        std::env::set_var("PI_EXPERIMENTAL", "yes");
        assert!(!are_experimental_features_enabled());

        std::env::set_var("PI_EXPERIMENTAL", "true");
        assert!(!are_experimental_features_enabled());

        // Cleanup
        std::env::remove_var("PI_EXPERIMENTAL");
    }
}
