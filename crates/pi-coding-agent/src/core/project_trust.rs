//! Project trust resolution — determines whether an agent session should
//! trust a project directory to load its resources and execute commands.
//!
//! Mirrors packages/coding-agent/src/core/project-trust.ts

use crate::core::trust_manager::{
    has_trust_requiring_project_resources, ProjectTrustStore,
};

/// Default trust policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultProjectTrust {
    Ask,
    Always,
    Never,
}

impl DefaultProjectTrust {
    pub fn as_str(&self) -> &'static str {
        match self {
            DefaultProjectTrust::Ask => "ask",
            DefaultProjectTrust::Always => "always",
            DefaultProjectTrust::Never => "never",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "always" => DefaultProjectTrust::Always,
            "never" => DefaultProjectTrust::Never,
            _ => DefaultProjectTrust::Ask,
        }
    }
}

/// Context object passed to event handlers when resolving project trust.
pub struct ProjectTrustContext {
    pub has_ui: bool,
    pub cwd: String,
}

impl ProjectTrustContext {
    pub fn new(cwd: &str, has_ui: bool) -> Self {
        ProjectTrustContext {
            has_ui,
            cwd: cwd.to_string(),
        }
    }
}

/// Options for resolving project trust.
pub struct ResolveProjectTrustedOptions<'a> {
    pub cwd: &'a str,
    pub trust_store: &'a ProjectTrustStore,
    /// If set, bypasses all other trust resolution.
    pub trust_override: Option<bool>,
    /// Global default trust policy.
    pub default_project_trust: DefaultProjectTrust,
    /// Context for UI interactions.
    pub project_trust_context: ProjectTrustContext,
}

/// Resolve whether a project is trusted.
///
/// Resolution order:
/// 1. `trust_override` (explicit caller override)
/// 2. Check if project has trust-requiring resources; if not, return `true`
/// 3. Check stored trust decisions (walking up directory tree)
/// 4. Apply `default_project_trust` policy
/// 5. If policy is "ask" and UI is available, prompt the user
/// 6. If no UI, return `false` (conservative default)
pub fn resolve_project_trusted(options: ResolveProjectTrustedOptions<'_>) -> bool {
    // 1. Explicit override
    if let Some(override_val) = options.trust_override {
        return override_val;
    }

    // 2. Check if trust is even needed
    if !has_trust_requiring_project_resources(options.cwd) {
        return true;
    }

    // 3. Check stored decisions
    if let Some(entry) = options.trust_store.get_entry(options.cwd) {
        if let Some(decision) = entry.decision {
            return decision;
        }
    }

    // 4. Apply global default
    match options.default_project_trust {
        DefaultProjectTrust::Always => return true,
        DefaultProjectTrust::Never => return false,
        DefaultProjectTrust::Ask => {}
    }

    // 5. Prompt user if UI available
    if !options.project_trust_context.has_ui {
        return false;
    }

    // In non-interactive mode without an actual TUI, the "ask" path falls through
    // to the conservative default. A GUI client would implement its own prompt.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_resolve_trust_override_true() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectTrustStore::new_from_path(dir.path().join("trust.json"));
        let ctx = ProjectTrustContext::new("/tmp", false);

        let trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
            cwd: "/tmp",
            trust_store: &store,
            trust_override: Some(true),
            default_project_trust: DefaultProjectTrust::Never,
            project_trust_context: ctx,
        });
        assert!(trusted);
    }

    #[test]
    fn test_resolve_trust_override_false() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectTrustStore::new_from_path(dir.path().join("trust.json"));
        let ctx = ProjectTrustContext::new("/tmp", false);

        let trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
            cwd: "/tmp",
            trust_store: &store,
            trust_override: Some(false),
            default_project_trust: DefaultProjectTrust::Always,
            project_trust_context: ctx,
        });
        assert!(!trusted);
    }

    #[test]
    fn test_resolve_trust_no_resources_needed() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectTrustStore::new_from_path(dir.path().join("trust.json"));
        let ctx = ProjectTrustContext::new(dir.path().to_str().unwrap(), false);

        // Fresh temp dir has no .pi/ resources, so trust is auto-granted
        let trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
            cwd: dir.path().to_str().unwrap(),
            trust_store: &store,
            trust_override: None,
            default_project_trust: DefaultProjectTrust::Ask,
            project_trust_context: ctx,
        });
        assert!(trusted);
    }

    #[test]
    fn test_resolve_trust_from_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectTrustStore::new_from_path(dir.path().join("trust.json"));

        // Create a .pi/extensions to make trust required
        let pi_ext = dir.path().join(crate::config::CONFIG_DIR_NAME).join("extensions");
        fs::create_dir_all(&pi_ext).unwrap();

        // Store a trust decision
        store.set(dir.path().to_str().unwrap(), Some(true));

        let ctx = ProjectTrustContext::new(dir.path().to_str().unwrap(), false);
        let trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
            cwd: dir.path().to_str().unwrap(),
            trust_store: &store,
            trust_override: None,
            default_project_trust: DefaultProjectTrust::Ask,
            project_trust_context: ctx,
        });
        assert!(trusted);
    }

    #[test]
    fn test_resolve_trust_default_never() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectTrustStore::new_from_path(dir.path().join("trust.json"));

        // Create .pi-rs/settings.json to make trust required
        let pi_dir = dir.path().join(crate::config::CONFIG_DIR_NAME);
        fs::create_dir_all(&pi_dir).unwrap();
        fs::write(pi_dir.join("settings.json"), "{}").unwrap();

        let ctx = ProjectTrustContext::new(dir.path().to_str().unwrap(), false);
        let trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
            cwd: dir.path().to_str().unwrap(),
            trust_store: &store,
            trust_override: None,
            default_project_trust: DefaultProjectTrust::Never,
            project_trust_context: ctx,
        });
        assert!(!trusted);
    }

    #[test]
    fn test_resolve_trust_no_ui_defaults_false() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectTrustStore::new_from_path(dir.path().join("trust.json"));

        // Create .pi/extensions to make trust required
        let pi_dir = dir.path().join(crate::config::CONFIG_DIR_NAME).join("extensions");
        fs::create_dir_all(&pi_dir).unwrap();

        let ctx = ProjectTrustContext::new(dir.path().to_str().unwrap(), false);
        let trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
            cwd: dir.path().to_str().unwrap(),
            trust_store: &store,
            trust_override: None,
            default_project_trust: DefaultProjectTrust::Ask,
            project_trust_context: ctx,
        });
        // No UI + "ask" → conservative: not trusted
        assert!(!trusted);
    }

    #[test]
    fn test_default_project_trust_conversion() {
        assert_eq!(DefaultProjectTrust::from_str("always"), DefaultProjectTrust::Always);
        assert_eq!(DefaultProjectTrust::from_str("never"), DefaultProjectTrust::Never);
        assert_eq!(DefaultProjectTrust::from_str("ask"), DefaultProjectTrust::Ask);
        assert_eq!(DefaultProjectTrust::from_str("unknown"), DefaultProjectTrust::Ask);
        assert_eq!(DefaultProjectTrust::Always.as_str(), "always");
        assert_eq!(DefaultProjectTrust::Never.as_str(), "never");
        assert_eq!(DefaultProjectTrust::Ask.as_str(), "ask");
    }
}
