//! Pi user-agent string generation.
//!
//! Mirrors packages/coding-agent/src/utils/pi-user-agent.ts

/// Generate the pi user-agent string.
pub fn get_pi_user_agent(version: &str) -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let runtime = "rust";
    format!("pi/{version} ({os}; {runtime}; {arch})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_agent_format() {
        let ua = get_pi_user_agent("1.0.0");
        assert!(ua.starts_with("pi/"));
        assert!(ua.contains("1.0.0"));
        assert!(ua.contains("rust"));
    }

    #[test]
    fn test_user_agent_not_empty() {
        let ua = get_pi_user_agent("1.78.0");
        assert!(!ua.is_empty());
    }
}
