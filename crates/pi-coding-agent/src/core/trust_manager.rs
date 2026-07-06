//! Project trust store — persists trust decisions for project directories.
//!
//! Mirrors packages/coding-agent/src/core/trust-manager.ts

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::config;

// ============================================================================
// Types
// ============================================================================

/// `true` = trusted, `false` = not trusted, `None` = no decision (cleared)
pub type ProjectTrustDecision = Option<bool>;

#[derive(Debug, Clone)]
pub struct ProjectTrustStoreEntry {
    pub path: String,
    pub decision: ProjectTrustDecision,
}

#[derive(Debug, Clone)]
pub struct ProjectTrustUpdate {
    pub path: String,
    pub decision: ProjectTrustDecision,
}

#[derive(Debug, Clone)]
pub struct ProjectTrustOption {
    pub label: String,
    pub trusted: bool,
    pub updates: Vec<ProjectTrustUpdate>,
    pub saved_path: Option<String>,
}

type TrustFileData = BTreeMap<String, Option<bool>>;

/// Directories/files under `{cwd}/.pi/` that require trust to load.
const TRUST_REQUIRING_PROJECT_RESOURCES: &[&str] = &[
    "settings.json",
    "extensions",
    "skills",
    "prompts",
    "themes",
    "SYSTEM.md",
    "APPEND_SYSTEM.md",
];

// ============================================================================
// Path normalization
// ============================================================================

fn normalize_cwd(cwd: &str) -> String {
    crate::config::resolve_path(cwd)
}

// ============================================================================
// Trust file I/O (with advisory file locking)
// ============================================================================

fn read_trust_file(path: &Path) -> Result<TrustFileData, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read trust store {:?}: {}", path, e))?;

    let parsed: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse trust store {:?}: {}", path, e))?;

    let obj = parsed
        .as_object()
        .ok_or_else(|| format!("Invalid trust store {:?}: expected an object", path))?;

    let mut data = TrustFileData::new();
    for (key, value) in obj {
        match value {
            serde_json::Value::Bool(b) => {
                data.insert(key.clone(), Some(*b));
            }
            serde_json::Value::Null => {
                data.insert(key.clone(), None);
            }
            _ => {
                return Err(format!(
                    "Invalid trust store {:?}: value for {} must be true, false, or null",
                    path,
                    serde_json::to_string(key).unwrap_or_else(|_| key.clone())
                ));
            }
        }
    }
    Ok(data)
}

fn write_trust_file(path: &Path, data: &TrustFileData) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create trust store directory {:?}: {}", parent, e))?;
    }

    // Build sorted JSON (BTreeMap ensures alphabetical keys)
    let sorted: TrustFileData = data
        .iter()
        .filter(|(_, v)| v.is_some()) // skip null entries
        .map(|(k, v)| (k.clone(), *v))
        .collect();

    let json = serde_json::to_string_pretty(&sorted)
        .map_err(|e| format!("Failed to serialize trust store: {}", e))?;

    // Write atomically via temp file
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, &json)
        .map_err(|e| format!("Failed to write trust store {:?}: {}", tmp_path, e))?;
    fs::rename(&tmp_path, path)
        .map_err(|e| format!("Failed to rename trust store {:?}: {}", path, e))?;

    Ok(())
}

/// Execute a closure with an exclusive file lock on the trust file.
fn with_trust_file_lock<T>(path: &Path, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    // Ensure parent directory exists for lock file
    let lock_dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(lock_dir)
        .map_err(|e| format!("Failed to create lock directory {:?}: {}", lock_dir, e))?;

    // Open (or create) the lock file
    let lock_path = path.with_extension("json.lock");
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("Failed to open lock file {:?}: {}", lock_path, e))?;

    // Acquire exclusive lock (retry logic like original)
    let max_attempts = 10;
    let delay = std::time::Duration::from_millis(20);

    for attempt in 1..=max_attempts {
        match lock_file.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) => {
                if attempt == max_attempts {
                    return Err(format!(
                        "Failed to acquire lock on {:?} after {} attempts: {}",
                        lock_path, max_attempts, e
                    ));
                }
                std::thread::sleep(delay);
                let _ = e; // suppress unused warning
            }
        }
    }

    // Execute the closure
    let result = f();

    // Release lock (file handle closes on drop, which releases the lock)
    let _ = lock_file.unlock();

    result
}

// ============================================================================
// Public API
// ============================================================================

/// Find the nearest trust entry by walking up the directory tree.
pub fn find_nearest_trust_entry(
    data: &TrustFileData,
    cwd: &str,
) -> Option<ProjectTrustStoreEntry> {
    let mut current = normalize_cwd(cwd);

    loop {
        if let Some(decision) = data.get(&current) {
            return Some(ProjectTrustStoreEntry {
                path: current.clone(),
                decision: *decision,
            });
        }

        let parent = Path::new(&current).parent()?;
        let parent_str = parent.to_string_lossy().to_string();
        if parent_str == current {
            return None;
        }
        current = parent_str;
    }
}

/// Get the parent directory path for trust inheritance.
pub fn get_project_trust_parent_path(cwd: &str) -> Option<String> {
    let trust_path = normalize_cwd(cwd);
    let parent = Path::new(&trust_path).parent()?;
    let parent_str = parent.to_string_lossy().to_string();
    if parent_str == trust_path {
        None
    } else {
        Some(parent_str)
    }
}

/// Generate trust options for the user to choose from (mirrors UI in original).
pub fn get_project_trust_options(
    cwd: &str,
    include_session_only: bool,
) -> Vec<ProjectTrustOption> {
    let trust_path = normalize_cwd(cwd);
    let mut options = Vec::new();

    // "Trust"
    options.push(ProjectTrustOption {
        label: "Trust".into(),
        trusted: true,
        updates: vec![ProjectTrustUpdate {
            path: trust_path.clone(),
            decision: Some(true),
        }],
        saved_path: Some(trust_path.clone()),
    });

    // "Trust parent folder"
    if let Some(parent_path) = get_project_trust_parent_path(cwd) {
        let pp_clone = parent_path.clone();
        options.push(ProjectTrustOption {
            label: format!("Trust parent folder ({})", parent_path),
            trusted: true,
            updates: vec![
                ProjectTrustUpdate {
                    path: parent_path,
                    decision: Some(true),
                },
                ProjectTrustUpdate {
                    path: trust_path.clone(),
                    decision: None,
                },
            ],
            saved_path: Some(pp_clone),
        });
    }

    // "Trust (this session only)" — updates list empty, so it's NOT persisted
    if include_session_only {
        options.push(ProjectTrustOption {
            label: "Trust (this session only)".into(),
            trusted: true,
            updates: vec![],
            saved_path: None,
        });
    }

    // "Do not trust"
    options.push(ProjectTrustOption {
        label: "Do not trust".into(),
        trusted: false,
        updates: vec![ProjectTrustUpdate {
            path: trust_path.clone(),
            decision: Some(false),
        }],
        saved_path: Some(trust_path.clone()),
    });

    // "Do not trust (this session only)"
    if include_session_only {
        options.push(ProjectTrustOption {
            label: "Do not trust (this session only)".into(),
            trusted: false,
            updates: vec![],
            saved_path: None,
        });
    }

    options
}

/// Check if a project directory has resources that require trust.
///
/// Returns `true` when `cwd` has trust-requiring resources under:
///   - `{cwd}/.pi/{settings.json, extensions, skills, prompts, themes, ...}`
///   - `{parent_cwd}/.agents/skills` in any ancestor (except user's own ~/.agents/skills)
pub fn has_trust_requiring_project_resources(cwd: &str) -> bool {
    let home_dir = normalize_cwd(
        &dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/tmp".into()),
    );
    let user_agents_skills = Path::new(&home_dir).join(".agents").join("skills");
    let mut current = normalize_cwd(cwd);

    // Check {cwd}/.pi/ for trust-requiring resources
    let config_dir = Path::new(&current).join(config::CONFIG_DIR_NAME);
    if TRUST_REQUIRING_PROJECT_RESOURCES
        .iter()
        .any(|entry| config_dir.join(entry).exists())
    {
        return true;
    }

    // Walk up ancestors checking .agents/skills
    loop {
        let agents_skills = Path::new(&current).join(".agents").join("skills");
        if agents_skills != user_agents_skills && agents_skills.exists() {
            return true;
        }

        let parent = Path::new(&current)
            .parent()
            .map(|p| p.to_string_lossy().to_string());
        match parent {
            Some(parent_str) if parent_str != current => current = parent_str,
            _ => break,
        }
    }

    false
}

// ============================================================================
// ProjectTrustStore
// ============================================================================

/// Persistent store for project trust decisions.
///
/// Data is stored in `{agentDir}/trust.json` with advisory file locking
/// for cross-process safety.
pub struct ProjectTrustStore {
    trust_path: PathBuf,
}

impl ProjectTrustStore {
    pub fn new(agent_dir: &str) -> Self {
        let trust_path = Path::new(agent_dir).join("trust.json");
        ProjectTrustStore { trust_path }
    }

    pub fn new_from_path(trust_path: PathBuf) -> Self {
        ProjectTrustStore { trust_path }
    }

    /// Get the trust decision for `cwd`, walking up parent directories if needed.
    pub fn get(&self, cwd: &str) -> ProjectTrustDecision {
        self.get_entry(cwd)
            .map(|e| e.decision)
            .flatten()
    }

    /// Get the trust entry (path + decision) for `cwd`.
    pub fn get_entry(&self, cwd: &str) -> Option<ProjectTrustStoreEntry> {
        with_trust_file_lock(&self.trust_path, || {
            let data = read_trust_file(&self.trust_path)?;
            Ok(find_nearest_trust_entry(&data, cwd))
        })
        .unwrap_or(None)
    }

    /// Set a trust decision for a specific path.
    pub fn set(&self, cwd: &str, decision: ProjectTrustDecision) {
        self.set_many(&[ProjectTrustUpdate {
            path: cwd.to_string(),
            decision,
        }]);
    }

    /// Apply multiple trust updates atomically.
    pub fn set_many(&self, decisions: &[ProjectTrustUpdate]) {
        let result = with_trust_file_lock(&self.trust_path, || {
            let mut data = read_trust_file(&self.trust_path)?;
            for update in decisions {
                let key = normalize_cwd(&update.path);
                match update.decision {
                    Some(b) => {
                        data.insert(key, Some(b));
                    }
                    None => {
                        data.remove(&key);
                    }
                }
            }
            write_trust_file(&self.trust_path, &data)?;
            Ok(())
        });

        if let Err(e) = result {
            eprintln!("[pi] Failed to write trust store: {e}");
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_nearest_trust_entry() {
        let mut data = TrustFileData::new();
        data.insert("/home/user/project".into(), Some(true));

        // Exact match
        let entry = find_nearest_trust_entry(&data, "/home/user/project");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().decision, Some(true));

        // No match — parent traversal doesn't find anything either
        let entry = find_nearest_trust_entry(&data, "/other");
        assert!(entry.is_none());
    }

    #[test]
    fn test_find_nearest_trust_entry_parent() {
        let mut data = TrustFileData::new();
        data.insert("/home/user".into(), Some(false));

        // Should find parent trust decision
        let entry = find_nearest_trust_entry(&data, "/home/user/project/subdir");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().decision, Some(false));
    }

    #[test]
    fn test_project_trust_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectTrustStore::new_from_path(dir.path().join("trust.json"));

        // Initially no decision
        assert_eq!(store.get("/tmp/test-project"), None);

        // Set trusted
        store.set("/tmp/test-project", Some(true));
        assert_eq!(store.get("/tmp/test-project"), Some(true));

        // Clear
        store.set("/tmp/test-project", None);
        assert_eq!(store.get("/tmp/test-project"), None);
    }

    #[test]
    fn test_has_trust_requiring_project_resources_no_resources() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        assert!(!has_trust_requiring_project_resources(&cwd));
    }

    #[test]
    fn test_has_trust_requiring_project_resources_with_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let pi_dir = dir.path().join(config::CONFIG_DIR_NAME).join("extensions");
        fs::create_dir_all(&pi_dir).unwrap();

        let cwd = dir.path().to_string_lossy().to_string();
        assert!(has_trust_requiring_project_resources(&cwd));
    }

    #[test]
    fn test_get_project_trust_options() {
        let options = get_project_trust_options("/some/project", true);
        assert_eq!(options.len(), 5); // Trust + Trust parent + Trust session + Don't trust + Don't trust session

        // First option should be "Trust"
        assert_eq!(options[0].label, "Trust");
        assert!(options[0].trusted);
    }

    #[test]
    fn test_get_project_trust_options_without_session() {
        let options = get_project_trust_options("/some/project", false);
        assert_eq!(options.len(), 3); // Trust + Trust parent + Don't trust
    }

    #[test]
    fn test_read_write_trust_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");

        let mut data = TrustFileData::new();
        data.insert("/a".into(), Some(true));
        data.insert("/b".into(), Some(false));

        write_trust_file(&path, &data).unwrap();
        let read_back = read_trust_file(&path).unwrap();

        assert_eq!(read_back.get("/a"), Some(&Some(true)));
        assert_eq!(read_back.get("/b"), Some(&Some(false)));
    }
}
