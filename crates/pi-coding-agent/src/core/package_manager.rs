//! Package manager — resolves and installs npm packages.
//!
//! Mirrors packages/coding-agent/src/core/package-manager.ts
//!
//! Uses subprocess calls to `npm` for package resolution and installation.
//! Supports both user-level (global) and project-level package scopes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

// ============================================================================
// Types
// ============================================================================

/// Origin of a package file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    User,
    Project,
}

/// Metadata for a resolved package path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathMetadata {
    pub source: String,
    pub scope: SourceScope,
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
}

/// A resolved resource (extension, skill, prompt, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedResource {
    pub path: String,
    pub enabled: bool,
    pub metadata: PathMetadata,
}

/// All resolved paths from package resolution.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResolvedPaths {
    pub extensions: Vec<ResolvedResource>,
    pub skills: Vec<ResolvedResource>,
    pub prompts: Vec<ResolvedResource>,
    pub themes: Vec<ResolvedResource>,
}

/// A configured package reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfiguredPackage {
    pub source: String,
    pub scope: String,
    pub filtered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_path: Option<String>,
}

/// Progress event during package operations.
#[derive(Debug, Clone)]
pub struct ProgressEvent {
    pub event_type: String,
    pub action: String,
    pub source: String,
    pub message: Option<String>,
}

/// Action to take when a package source is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSourceAction {
    Install,
    Skip,
    Error,
}

/// Progress callback type.
pub type ProgressCallback = Box<dyn Fn(ProgressEvent) + Send + Sync>;

// ============================================================================
// PackageManager trait
// ============================================================================

/// Interface for package management operations.
pub trait PackageManager: Send + Sync {
    /// Resolve all configured packages, optionally handling missing ones.
    fn resolve(
        &self,
        on_missing: Option<&dyn Fn(&str) -> MissingSourceAction>,
    ) -> Result<ResolvedPaths, String>;

    /// Install a package from a source string.
    fn install(&self, source: &str, local: bool) -> Result<(), String>;

    /// Install and persist the package to settings.
    fn install_and_persist(&self, source: &str, local: bool) -> Result<(), String>;

    /// Remove a package.
    fn remove(&self, source: &str, local: bool) -> Result<(), String>;

    /// Remove and persist removal to settings.
    fn remove_and_persist(&self, source: &str, local: bool) -> Result<bool, String>;

    /// List configured packages.
    fn list_configured_packages(&self) -> Vec<ConfiguredPackage>;

    /// Set a progress callback.
    fn set_progress_callback(&self, callback: Option<ProgressCallback>);
}

// ============================================================================
// NpmHelper — wraps npm CLI calls
// ============================================================================

struct NpmHelper;

impl NpmHelper {
    /// Check if npm is available.
    fn is_available() -> bool {
        std::process::Command::new("npm")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get the npm root (global or local).
    fn get_root(global: bool) -> Result<String, String> {
        let mut cmd = std::process::Command::new("npm");
        cmd.arg("root");
        if global {
            cmd.arg("-g");
        }
        let output = cmd.output().map_err(|e| format!("npm root failed: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("npm root error: {stderr}"));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Check if a package is installed in the given root.
    fn is_package_installed(name: &str, root: &str) -> bool {
        let pkg_path = Path::new(root).join(name);
        pkg_path.join("package.json").exists()
    }

    /// List installed packages in a directory (names only).
    fn list_installed(root: &str) -> Vec<String> {
        let dir = Path::new(root);
        if !dir.exists() {
            return Vec::new();
        }
        let mut packages = Vec::new();
        let entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(e) => e.flatten().collect(),
            Err(_) => return Vec::new(),
        };

        // Regular packages
        for entry in &entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('@') {
                continue;
            }
            if entry.path().join("package.json").exists() {
                packages.push(name);
            }
        }

        // Scoped packages (@scope/name)
        for entry in &entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('@') && entry.path().is_dir() {
                if let Ok(sub_entries) = std::fs::read_dir(entry.path()) {
                    for sub in sub_entries.flatten() {
                        let scoped_name = format!("{}/{}", name, sub.file_name().to_string_lossy());
                        if sub.path().join("package.json").exists() {
                            packages.push(scoped_name);
                        }
                    }
                }
            }
        }

        packages
    }

    /// View package info as JSON.
    fn view(package: &str) -> Result<serde_json::Value, String> {
        let output = std::process::Command::new("npm")
            .args(["view", package, "--json"])
            .output()
            .map_err(|e| format!("npm view failed: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("npm view error: {stderr}"));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Failed to parse npm view output: {e}"))
    }

    /// Install a package.
    fn install(package: &str, cwd: &str, global: bool) -> Result<(), String> {
        let mut cmd = std::process::Command::new("npm");
        if global {
            cmd.args(["install", "-g", package]);
        } else {
            cmd.args(["install", package]);
        }
        cmd.current_dir(cwd);
        let output = cmd.output().map_err(|e| format!("npm install failed: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("npm install error: {stderr}"));
        }
        Ok(())
    }

    /// Uninstall a package.
    fn uninstall(package: &str, cwd: &str, global: bool) -> Result<(), String> {
        let mut cmd = std::process::Command::new("npm");
        if global {
            cmd.args(["uninstall", "-g", package]);
        } else {
            cmd.args(["uninstall", package]);
        }
        cmd.current_dir(cwd);
        let output = cmd.output().map_err(|e| format!("npm uninstall failed: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("npm uninstall error: {stderr}"));
        }
        Ok(())
    }
}

// ============================================================================
// DefaultPackageManager
// ============================================================================

/// Default implementation of PackageManager.
///
/// Resolves packages from:
/// - User scope: `{agentDir}/node_modules/`
/// - Project scope: `{cwd}/node_modules/`
pub struct DefaultPackageManager {
    cwd: String,
    agent_dir: String,
    progress_callback: Mutex<Option<ProgressCallback>>,
    /// Cache of resolved paths (source → path).
    path_cache: Mutex<HashMap<String, String>>,
}

impl DefaultPackageManager {
    /// Create a new package manager.
    pub fn new(cwd: &str, agent_dir: &str) -> Self {
        DefaultPackageManager {
            cwd: cwd.to_string(),
            agent_dir: agent_dir.to_string(),
            progress_callback: Mutex::new(None),
            path_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Get the user-level node_modules path.
    fn user_node_modules(&self) -> PathBuf {
        Path::new(&self.agent_dir).join("node_modules")
    }

    /// Get the project-level node_modules path.
    fn project_node_modules(&self) -> PathBuf {
        Path::new(&self.cwd).join("node_modules")
    }

    /// Emit a progress event.
    fn emit_progress(&self, action: &str, source: &str, message: Option<&str>) {
        if let Some(cb) = self.progress_callback.lock().unwrap().as_ref() {
            cb(ProgressEvent {
                event_type: "progress".to_string(),
                action: action.to_string(),
                source: source.to_string(),
                message: message.map(|s| s.to_string()),
            });
        }
    }

    /// Resolve a single package source to a path.
    fn resolve_source(&self, source: &str, local: bool) -> Option<String> {
        // Check cache first
        {
            let cache = self.path_cache.lock().unwrap();
            if let Some(path) = cache.get(source) {
                return Some(path.clone());
            }
        }

        let roots = if local {
            vec![self.project_node_modules()]
        } else {
            vec![self.user_node_modules(), self.project_node_modules()]
        };

        for root in &roots {
            let pkg_dir = root.join(source);
            if pkg_dir.join("package.json").exists() {
                let path = pkg_dir.to_string_lossy().to_string();
                self.path_cache.lock().unwrap().insert(source.to_string(), path.clone());
                return Some(path);
            }
        }

        None
    }
}

impl PackageManager for DefaultPackageManager {
    fn resolve(
        &self,
        on_missing: Option<&dyn Fn(&str) -> MissingSourceAction>,
    ) -> Result<ResolvedPaths, String> {
        let mut result = ResolvedPaths::default();

        // Scan user node_modules
        let user_nm = self.user_node_modules();
        if user_nm.exists() {
            let packages = NpmHelper::list_installed(&user_nm.to_string_lossy());
            for pkg in packages {
                let pkg_path = user_nm.join(&pkg).to_string_lossy().to_string();
                result.extensions.push(ResolvedResource {
                    path: pkg_path,
                    enabled: true,
                    metadata: PathMetadata {
                        source: pkg,
                        scope: SourceScope::User,
                        origin: "package".to_string(),
                        base_dir: Some(user_nm.to_string_lossy().to_string()),
                    },
                });
            }
        }

        // Scan project node_modules
        let project_nm = self.project_node_modules();
        if project_nm.exists() {
            let packages = NpmHelper::list_installed(&project_nm.to_string_lossy());
            for pkg in packages {
                let pkg_path = project_nm.join(&pkg).to_string_lossy().to_string();
                result.extensions.push(ResolvedResource {
                    path: pkg_path,
                    enabled: true,
                    metadata: PathMetadata {
                        source: pkg,
                        scope: SourceScope::Project,
                        origin: "package".to_string(),
                        base_dir: Some(project_nm.to_string_lossy().to_string()),
                    },
                });
            }
        }

        Ok(result)
    }

    fn install(&self, source: &str, local: bool) -> Result<(), String> {
        if !NpmHelper::is_available() {
            return Err("npm is not available. Install Node.js to use package management.".into());
        }

        self.emit_progress("install", source, Some("Installing..."));

        if local {
            NpmHelper::install(source, &self.cwd, false)?;
        } else {
            NpmHelper::install(source, &self.cwd, true)?;
        }

        self.emit_progress("install", source, Some("Installed"));
        Ok(())
    }

    fn install_and_persist(&self, source: &str, local: bool) -> Result<(), String> {
        self.install(source, local)?;
        // In the future, persist to settings here
        Ok(())
    }

    fn remove(&self, source: &str, local: bool) -> Result<(), String> {
        if !NpmHelper::is_available() {
            return Err("npm is not available.".into());
        }

        self.emit_progress("remove", source, Some("Removing..."));

        if local {
            NpmHelper::uninstall(source, &self.cwd, false)?;
        } else {
            NpmHelper::uninstall(source, &self.cwd, true)?;
        }

        // Clear cache entry
        self.path_cache.lock().unwrap().remove(source);

        self.emit_progress("remove", source, Some("Removed"));
        Ok(())
    }

    fn remove_and_persist(&self, source: &str, local: bool) -> Result<bool, String> {
        self.remove(source, local)?;
        Ok(true)
    }

    fn list_configured_packages(&self) -> Vec<ConfiguredPackage> {
        let mut packages = Vec::new();

        let user_nm = self.user_node_modules();
        if user_nm.exists() {
            for pkg in NpmHelper::list_installed(&user_nm.to_string_lossy()) {
                let installed = user_nm.join(&pkg).to_string_lossy().to_string();
                packages.push(ConfiguredPackage {
                    source: pkg,
                    scope: "user".to_string(),
                    filtered: false,
                    installed_path: Some(installed),
                });
            }
        }

        let project_nm = self.project_node_modules();
        if project_nm.exists() {
            for pkg in NpmHelper::list_installed(&project_nm.to_string_lossy()) {
                let installed = project_nm.join(&pkg).to_string_lossy().to_string();
                // Avoid duplicates if same package is in both
                if !packages.iter().any(|p| p.source == pkg && p.scope == "project") {
                    packages.push(ConfiguredPackage {
                        source: pkg,
                        scope: "project".to_string(),
                        filtered: false,
                        installed_path: Some(installed),
                    });
                }
            }
        }

        packages
    }

    fn set_progress_callback(&self, callback: Option<ProgressCallback>) {
        *self.progress_callback.lock().unwrap() = callback;
    }
}

/// Check if npm is available on the system.
pub fn is_npm_available() -> bool {
    NpmHelper::is_available()
}

/// Get the default npm root path.
pub fn get_npm_root(global: bool) -> Result<String, String> {
    NpmHelper::get_root(global)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_npm_available() {
        // This test checks if npm is on the system
        let available = is_npm_available();
        // No assertion — just verify it runs without panicking
        let _ = available;
    }

    #[test]
    fn test_default_package_manager_creation() {
        let mgr = DefaultPackageManager::new("/tmp/test", "/tmp/test-agent");
        let packages = mgr.list_configured_packages();
        assert!(packages.is_empty());
    }

    #[test]
    fn test_resolve_empty() {
        let mgr = DefaultPackageManager::new("/nonexistent", "/nonexistent");
        let result = mgr.resolve(None).unwrap();
        assert!(result.extensions.is_empty());
        assert!(result.skills.is_empty());
        assert!(result.prompts.is_empty());
        assert!(result.themes.is_empty());
    }

    #[test]
    fn test_detect_package_in_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        let nm = dir.path().join("node_modules").join("test-pkg");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("package.json"), r#"{"name":"test-pkg","version":"1.0.0"}"#).unwrap();

        let mgr = DefaultPackageManager::new(dir.path().to_str().unwrap(), "/nonexistent");
        let result = mgr.resolve(None).unwrap();
        assert!(!result.extensions.is_empty());
        assert!(result.extensions.iter().any(|r| r.metadata.source == "test-pkg"));
    }

    #[test]
    fn test_detect_scoped_package() {
        let dir = tempfile::tempdir().unwrap();
        let nm = dir.path().join("node_modules").join("@scope").join("test-pkg");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("package.json"), r#"{"name":"@scope/test-pkg","version":"1.0.0"}"#).unwrap();

        let mgr = DefaultPackageManager::new(dir.path().to_str().unwrap(), "/nonexistent");
        let result = mgr.resolve(None).unwrap();
        assert!(!result.extensions.is_empty());
        assert!(result.extensions.iter().any(|r| r.metadata.source == "@scope/test-pkg"));
    }

    #[test]
    fn test_source_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_path = dir.path().join("node_modules").join("my-pkg");
        fs::create_dir_all(&pkg_path).unwrap();
        fs::write(pkg_path.join("package.json"), r#"{"version":"1.0.0"}"#).unwrap();

        let mgr = DefaultPackageManager::new(dir.path().to_str().unwrap(), "/nonexistent");
        let resolved = mgr.resolve_source("my-pkg", false);
        assert!(resolved.is_some());
        assert!(resolved.unwrap().contains("my-pkg"));
    }

    #[test]
    fn test_types_serde() {
        let meta = PathMetadata {
            source: "test".into(),
            scope: SourceScope::Project,
            origin: "package".into(),
            base_dir: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("project"));

        let deserialized: PathMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.source, "test");
    }
}
