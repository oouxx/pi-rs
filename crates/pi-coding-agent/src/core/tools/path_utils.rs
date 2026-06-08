use std::path::{Path, PathBuf};

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

pub fn expand_path(path: &str) -> PathBuf {
    let expanded = if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            if path == "~" {
                home.to_string_lossy().to_string()
            } else if path.starts_with("~/") {
                format!("{}/{}", home.display(), &path[2..])
            } else {
                path.to_string()
            }
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };
    PathBuf::from(expanded)
}

pub fn resolve_to_cwd(path: &str, cwd: &str) -> PathBuf {
    let expanded = expand_path(path);
    if expanded.is_absolute() {
        expanded
    } else {
        PathBuf::from(cwd).join(expanded)
    }
}

pub fn resolve_read_path(path: &str, cwd: &str) -> PathBuf {
    let resolved = resolve_to_cwd(path, cwd);
    if resolved.exists() {
        return resolved;
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_path_tilde() {
        let result = expand_path("~/test");
        assert!(result.to_string_lossy().contains("test"));
    }

    #[test]
    fn test_resolve_to_cwd_relative() {
        let result = resolve_to_cwd("src/main.rs", "/home/user/project");
        assert_eq!(result, PathBuf::from("/home/user/project/src/main.rs"));
    }

    #[test]
    fn test_resolve_to_cwd_absolute() {
        let result = resolve_to_cwd("/usr/bin/test", "/home/user/project");
        assert_eq!(result, PathBuf::from("/usr/bin/test"));
    }
}
