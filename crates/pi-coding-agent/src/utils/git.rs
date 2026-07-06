//! Git URL parsing utilities.
//!
//! Mirrors packages/coding-agent/src/utils/git.ts

use std::sync::LazyLock;
use regex::Regex;

/// Parsed git source information.
#[derive(Debug, Clone)]
pub struct GitSource {
    pub repo: String,
    pub host: String,
    pub path: String,
    pub r#ref: Option<String>,
    pub pinned: bool,
}

static SCP_LIKE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^git@([^:]+):(.+)$").unwrap());

static PROTOCOL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(https?|ssh|git)://").unwrap());

/// Split a git URL into repo and optional ref.
fn split_ref(url: &str) -> (String, Option<String>) {
    // SCP-like (git@github.com:user/repo@ref)
    if let Some(caps) = SCP_LIKE_RE.captures(url) {
        let path_part = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if let Some(at_pos) = path_part.rfind('@') {
            let repo_path = &path_part[..at_pos];
            let ref_str = &path_part[at_pos + 1..];
            if !repo_path.is_empty() && !ref_str.is_empty() {
                let host = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                return (format!("git@{host}:{repo_path}"), Some(ref_str.to_string()));
            }
        }
        return (url.to_string(), None);
    }

    // Protocol URLs: find @ in the path portion (after first / after ://)
    if url.contains("://") {
        if let Some(scheme_end) = url.find("://") {
            let after_scheme = &url[scheme_end + 3..];
            // Find first / to separate authority from path
            if let Some(path_start) = after_scheme.find('/') {
                let authority = &after_scheme[..path_start];
                let path_and_ref = &after_scheme[path_start + 1..];
                // Check for @ in the path (indicating a ref)
                if let Some(at_pos) = path_and_ref.rfind('@') {
                    let repo_path = &path_and_ref[..at_pos];
                    let ref_str = &path_and_ref[at_pos + 1..];
                    if !repo_path.is_empty() && !ref_str.is_empty() {
                        let repo = format!("{}://{}/{}", &url[..scheme_end], authority, repo_path);
                        return (repo, Some(ref_str.to_string()));
                    }
                }
            }
        }
        return (url.to_string(), None);
    }

    // host/path form
    if let Some(slash_pos) = url.find('/') {
        let host_part = &url[..slash_pos];
        let path_part = &url[slash_pos + 1..];
        if let Some(at_pos) = path_part.rfind('@') {
            let repo_path = &path_part[..at_pos];
            let ref_str = &path_part[at_pos + 1..];
            if !repo_path.is_empty() && !ref_str.is_empty() {
                return (format!("{}/{}", host_part, repo_path), Some(ref_str.to_string()));
            }
        }
    }

    (url.to_string(), None)
}

/// Check if a string has unsafe characters for git install.
fn has_unsafe_part(value: &str, allow_slash: bool) -> bool {
    if value.contains('\0') || value.contains('\\') || value.starts_with('/') {
        return true;
    }
    if !allow_slash && value.contains('/') {
        return true;
    }
    if value.split('/').any(|part| part == "..") {
        return true;
    }
    false
}

/// Build a GitSource from validated parts.
fn build_git_source(repo: String, host: String, path: String, r#ref: Option<String>) -> Option<GitSource> {
    if path.starts_with('/') {
        return None;
    }
    let normalized_path = path.trim_end_matches(".git").to_string();
    if host.is_empty() || normalized_path.is_empty() || normalized_path.split('/').count() < 2 {
        return None;
    }
    if has_unsafe_part(&host, false) || has_unsafe_part(&normalized_path, true) {
        return None;
    }

    Some(GitSource {
        repo,
        host,
        path: normalized_path,
        r#ref: r#ref.clone(),
        pinned: r#ref.is_some(),
    })
}

/// Parse generic git URL (non-hosted).
fn parse_generic_git_url(url: &str) -> Option<GitSource> {
    let (repo_without_ref, r#ref) = split_ref(url);

    let (host, path) = if let Some(caps) = SCP_LIKE_RE.captures(&repo_without_ref) {
        let h = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        let p = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
        (h, p)
    } else if repo_without_ref.starts_with("https://")
        || repo_without_ref.starts_with("http://")
        || repo_without_ref.starts_with("ssh://")
        || repo_without_ref.starts_with("git://")
    {
        let prefixes = &["https://", "http://", "ssh://", "git://"];
        let rest = prefixes
            .iter()
            .find(|p| repo_without_ref.starts_with(**p))
            .map(|p| &repo_without_ref[p.len()..])
            .unwrap_or(&repo_without_ref);
        match rest.split_once('/') {
            Some((h, p)) => (h.to_string(), p.to_string()),
            None => (rest.to_string(), String::new()),
        }
    } else {
        let slash_idx = repo_without_ref.find('/')?;
        let h = repo_without_ref[..slash_idx].to_string();
        let p = repo_without_ref[slash_idx + 1..].to_string();
        if !h.contains('.') && h != "localhost" {
            return None;
        }
        (h, p)
    };

    let repo = if repo_without_ref.starts_with("git@")
        || repo_without_ref.starts_with("http")
        || repo_without_ref.starts_with("ssh")
        || repo_without_ref.starts_with("git://")
    {
        repo_without_ref.clone()
    } else {
        format!("https://{}", repo_without_ref)
    };

    build_git_source(repo, host, path, r#ref)
}

/// Parse a git source string into a `GitSource`.
pub fn parse_git_url(source: &str) -> Option<GitSource> {
    let trimmed = source.trim();
    let has_git_prefix = trimmed.starts_with("git:");
    let url = if has_git_prefix {
        trimmed[4..].trim()
    } else {
        trimmed
    };

    if !has_git_prefix && !PROTOCOL_RE.is_match(url) {
        return None;
    }

    parse_generic_git_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_https() {
        let result = parse_git_url("https://github.com/user/repo.git");
        assert!(result.is_some());
        let gs = result.unwrap();
        assert!(gs.repo.contains("github.com"));
        assert_eq!(gs.host, "github.com");
        assert_eq!(gs.path, "user/repo");
        assert!(!gs.pinned);
    }

    #[test]
    fn test_parse_with_ref() {
        let result = parse_git_url("https://github.com/user/repo.git@v1.0");
        assert!(result.is_some());
        let gs = result.unwrap();
        assert_eq!(gs.r#ref, Some("v1.0".into()));
        assert!(gs.pinned);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse_git_url("not-a-url").is_none());
        assert!(parse_git_url("").is_none());
    }

    #[test]
    fn test_parse_git_prefix() {
        let result = parse_git_url("git:https://github.com/user/repo.git");
        assert!(result.is_some());
    }

    #[test]
    fn test_split_ref() {
        let (repo, ref_opt) = split_ref("https://github.com/user/repo.git@v2.0");
        assert_eq!(ref_opt, Some("v2.0".into()));
        assert!(!repo.contains("@v2.0"));
    }

    #[test]
    fn test_unsafe_chars() {
        assert!(has_unsafe_part("hello\x00world", false));
        assert!(!has_unsafe_part("hello/world", true));
        assert!(has_unsafe_part("hello/world", false));
        assert!(has_unsafe_part("/start", false));
    }
}
