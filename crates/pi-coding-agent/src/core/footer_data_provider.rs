use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

struct GitPaths {
    repo_dir: String,
    head_path: String,
}

fn find_git_paths(cwd: &str) -> Option<GitPaths> {
    let mut dir: &Path = Path::new(cwd);
    loop {
        let git_path = dir.join(".git");
        if git_path.exists() {
            let head_path = dir.join(".git").join("HEAD");
            if head_path.exists() {
                return Some(GitPaths {
                    repo_dir: dir.to_string_lossy().to_string(),
                    head_path: head_path.to_string_lossy().to_string(),
                });
            }
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return None,
        }
    }
}

fn read_git_head(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn resolve_branch_from_head(head_content: &str) -> Option<String> {
    if head_content.starts_with("ref: refs/heads/") {
        let branch = head_content.trim_start_matches("ref: refs/heads/");
        if branch == ".invalid" {
            None
        } else {
            Some(branch.to_string())
        }
    } else {
        Some("detached".to_string())
    }
}

pub struct FooterDataProvider {
    git_paths: Option<GitPaths>,
    cached_branch: Option<Option<String>>,
    extension_statuses: HashMap<String, String>,
    available_provider_count: Arc<AtomicUsize>,
    disposed: AtomicBool,
}

impl FooterDataProvider {
    pub fn new(cwd: &str) -> Self {
        let git_paths = find_git_paths(cwd);
        Self {
            cached_branch: git_paths.as_ref().map(|_| None),
            git_paths,
            extension_statuses: HashMap::new(),
            available_provider_count: Arc::new(AtomicUsize::new(0)),
            disposed: AtomicBool::new(false),
        }
    }

    pub fn get_git_branch(&self) -> Option<String> {
        let result = match &self.git_paths {
            Some(paths) => {
                let head = read_git_head(&paths.head_path)?;
                resolve_branch_from_head(&head)
            }
            None => None,
        };
        result
    }

    pub fn get_extension_statuses(&self) -> &HashMap<String, String> {
        &self.extension_statuses
    }

    pub fn set_extension_status(&mut self, key: &str, text: Option<&str>) {
        match text {
            Some(t) => {
                self.extension_statuses
                    .insert(key.to_string(), t.to_string());
            }
            None => {
                self.extension_statuses.remove(key);
            }
        }
    }

    pub fn clear_extension_statuses(&mut self) {
        self.extension_statuses.clear();
    }

    pub fn get_available_provider_count(&self) -> usize {
        self.available_provider_count.load(Ordering::Relaxed)
    }

    pub fn set_available_provider_count(&self, count: usize) {
        self.available_provider_count
            .store(count, Ordering::Relaxed);
    }

    pub fn dispose(&self) {
        self.disposed.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_git_repo() {
        let provider = FooterDataProvider::new("/nonexistent");
        assert!(provider.get_git_branch().is_none());
    }

    #[test]
    fn test_extension_statuses() {
        let mut provider = FooterDataProvider::new("/tmp");
        provider.set_extension_status("key1", Some("value1"));
        assert_eq!(
            provider
                .get_extension_statuses()
                .get("key1")
                .map(|s| s.as_str()),
            Some("value1")
        );
        provider.set_extension_status("key1", None);
        assert!(!provider.get_extension_statuses().contains_key("key1"));
    }

    #[test]
    fn test_provider_count() {
        let provider = FooterDataProvider::new("/tmp");
        assert_eq!(provider.get_available_provider_count(), 0);
        provider.set_available_provider_count(5);
        assert_eq!(provider.get_available_provider_count(), 5);
    }

    #[test]
    fn test_resolve_branch_from_head() {
        assert_eq!(
            resolve_branch_from_head("ref: refs/heads/main"),
            Some("main".to_string())
        );
        assert_eq!(
            resolve_branch_from_head("abc123"),
            Some("detached".to_string())
        );
    }
}
