//! Path resolution utilities.
//!
//! Mirrors packages/coding-agent/src/utils/paths.ts

use std::path::Path;

/// Unicode space characters to normalize.
const UNICODE_SPACES: &[char] = &[
    '\u{00A0}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}',
    '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}',
    '\u{202F}', '\u{205F}', '\u{3000}',
];

/// Options for path normalization.
#[derive(Debug, Clone)]
pub struct PathOptions {
    pub trim: bool,
    pub expand_tilde: bool,
    pub strip_at_prefix: bool,
    pub normalize_unicode_spaces: bool,
    pub home_dir: Option<String>,
}

impl Default for PathOptions {
    fn default() -> Self {
        PathOptions {
            trim: false,
            expand_tilde: true,
            strip_at_prefix: false,
            normalize_unicode_spaces: false,
            home_dir: None,
        }
    }
}

/// Normalize a path string (tilde expansion, unicode space normalization).
pub fn normalize_path(input: &str, options: &PathOptions) -> String {
    let mut normalized = if options.trim {
        input.trim().to_string()
    } else {
        input.to_string()
    };

    if options.normalize_unicode_spaces {
        normalized = normalized.replace(UNICODE_SPACES, " ");
    }

    if options.strip_at_prefix && normalized.starts_with('@') {
        normalized = normalized[1..].to_string();
    }

    if options.expand_tilde {
        let home = options.home_dir.clone().or_else(|| {
            dirs::home_dir().map(|p| p.to_string_lossy().to_string())
        }).unwrap_or_else(|| "/tmp".to_string());

        if normalized == "~" {
            return home;
        }
        if normalized.starts_with("~/") {
            return Path::new(&home).join(&normalized[2..]).to_string_lossy().to_string();
        }
    }

    normalized
}

/// Resolve a path, relative to `base_dir` if not absolute.
pub fn resolve_path(input: &str, base_dir: &str, options: &PathOptions) -> String {
    let normalized = normalize_path(input, options);
    if Path::new(&normalized).is_absolute() {
        normalized
    } else {
        let base = Path::new(base_dir);
        base.join(&normalized).to_string_lossy().to_string()
    }
}

/// Canonicalize a path (resolve symlinks), falling back to the raw path on failure.
pub fn canonicalize_path(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => path.to_string(),
    }
}

/// Check if a value is a local path (not npm:, git:, http:, etc.).
pub fn is_local_path(value: &str) -> bool {
    let trimmed = value.trim();
    !(trimmed.starts_with("npm:")
        || trimmed.starts_with("git:")
        || trimmed.starts_with("github:")
        || trimmed.starts_with("http:")
        || trimmed.starts_with("https:")
        || trimmed.starts_with("ssh:"))
}

/// Get a path relative to `cwd`, returning `None` if outside.
pub fn get_cwd_relative_path(file_path: &str, cwd: &str) -> Option<String> {
    use std::path::Path;
    let opts = PathOptions::default();
    let resolved_cwd = resolve_path(cwd, cwd, &opts);
    let resolved_path = resolve_path(file_path, &resolved_cwd, &opts);

    let relative = Path::new(&resolved_path)
        .strip_prefix(&resolved_cwd)
        .ok()?
        .to_string_lossy()
        .to_string();

    if relative.is_empty() {
        Some(".".to_string())
    } else if relative.starts_with("..") {
        None
    } else {
        Some(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_tilde() {
        let result = normalize_path("~/test", &PathOptions::default());
        assert!(!result.starts_with('~'));
        assert!(result.ends_with("/test"));
    }

    #[test]
    fn test_normalize_path_tilde_exact() {
        let result = normalize_path("~", &PathOptions::default());
        assert!(!result.starts_with('~'));
        assert!(!result.is_empty());
    }

    #[test]
    fn test_is_local_path() {
        assert!(is_local_path("./foo"));
        assert!(is_local_path("/absolute/path"));
        assert!(!is_local_path("npm:package"));
        assert!(!is_local_path("https://example.com"));
        assert!(!is_local_path("git:github.com/user/repo"));
    }

    #[test]
    fn test_canonicalize_path() {
        let result = canonicalize_path("/nonexistent/path");
        assert_eq!(result, "/nonexistent/path");
    }

    #[test]
    fn test_get_cwd_relative_path() {
        let result = get_cwd_relative_path("/tmp/test/file.txt", "/tmp/test");
        assert!(result.is_some());

        let outside = get_cwd_relative_path("/other/file.txt", "/tmp/test");
        assert!(outside.is_none() || outside.as_deref() == Some("../other/file.txt"));
    }

    #[test]
    fn test_resolve_path_absolute() {
        let result = resolve_path("/foo/bar", "/tmp", &PathOptions::default());
        assert_eq!(result, "/foo/bar");
    }

    #[test]
    fn test_resolve_path_relative() {
        let result = resolve_path("bar", "/foo", &PathOptions::default());
        assert!(result.contains("bar"));
    }
}
