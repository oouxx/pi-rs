//! Version checking — compare and check for newer package versions.
//!
//! Mirrors packages/coding-agent/src/utils/version-check.ts

use std::time::Duration;

use crate::utils::pi_user_agent::get_pi_user_agent;

/// Info about the latest release.
#[derive(Debug, Clone)]
pub struct LatestRelease {
    pub version: String,
    pub package_name: Option<String>,
    pub note: Option<String>,
}

/// Compare two semver versions.
/// Returns `None` if either version is invalid.
pub fn compare_package_versions(left: &str, right: &str) -> Option<i32> {
    let left = left.trim();
    let right = right.trim();

    let left_parts: Vec<&str> = left.split('.').collect();
    let right_parts: Vec<&str> = right.split('.').collect();

    if left_parts.len() != 3 || right_parts.len() != 3 {
        return None;
    }

    for i in 0..3 {
        let l: i32 = left_parts[i].parse().ok()?;
        let r: i32 = right_parts[i].parse().ok()?;
        if l != r {
            return Some(if l > r { 1 } else { -1 });
        }
    }

    Some(0)
}

/// Check if a candidate version is newer than the current version.
pub fn is_newer_package_version(candidate: &str, current: &str) -> bool {
    match compare_package_versions(candidate, current) {
        Some(cmp) => cmp > 0,
        None => candidate.trim() != current.trim(),
    }
}

/// Fetch the latest pi release version from the registry.
pub async fn get_latest_pi_release(current_version: &str) -> Result<LatestRelease, String> {
    let ua = get_pi_user_agent(current_version);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(&ua)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let resp = client
        .get("https://pi.dev/api/latest-version")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch latest version: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    Ok(LatestRelease {
        version: data["version"]
            .as_str()
            .unwrap_or(current_version)
            .to_string(),
        package_name: data["packageName"].as_str().map(|s| s.to_string()),
        note: data["note"].as_str().map(|s| s.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_equal() {
        assert_eq!(compare_package_versions("1.0.0", "1.0.0"), Some(0));
    }

    #[test]
    fn test_compare_newer() {
        assert_eq!(compare_package_versions("2.0.0", "1.0.0"), Some(1));
        assert_eq!(compare_package_versions("1.1.0", "1.0.0"), Some(1));
        assert_eq!(compare_package_versions("1.0.1", "1.0.0"), Some(1));
    }

    #[test]
    fn test_compare_older() {
        assert_eq!(compare_package_versions("1.0.0", "2.0.0"), Some(-1));
    }

    #[test]
    fn test_compare_invalid() {
        assert_eq!(compare_package_versions("abc", "1.0.0"), None);
        assert_eq!(compare_package_versions("1.0", "1.0.0"), None);
    }

    #[test]
    fn test_is_newer() {
        assert!(is_newer_package_version("2.0.0", "1.0.0"));
        assert!(!is_newer_package_version("1.0.0", "2.0.0"));
        assert!(!is_newer_package_version("1.0.0", "1.0.0"));
    }

    #[test]
    fn test_is_newer_fallback() {
        // Different strings that don't parse as semver
        assert!(is_newer_package_version("dev", "1.0.0"));
    }
}
