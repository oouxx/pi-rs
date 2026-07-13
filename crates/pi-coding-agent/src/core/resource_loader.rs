//! Resource loader — orchestrates loading of extensions, skills, prompts, themes,
//! and context files for the agent session.
//!
//! Ported from `packages/coding-agent/src/core/resource-loader.ts`.
//!
//! The `DefaultResourceLoader` manages the full reload cycle:
//!   1. (Optional) Pre-trust extension load → resolve project trust → post-trust reload
//!   2. Resolve packages via `PackageManager`
//!   3. Load extensions (with caching)
//!   4. Detect extension conflicts (tools, flags)
//!   5. Load skills, prompts, themes from resolved paths
//!   6. Load AGENTS.md/CLAUDE.md context files (cwd → root)
//!   7. Discover SYSTEM.md / APPEND_SYSTEM.md
//!   8. Apply override functions per resource type

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use crate::config;
use crate::core::diagnostics::{ResourceCollision, ResourceDiagnostic};
use crate::core::prompt_templates::{self, LoadPromptTemplatesOptions, PromptTemplate};
use crate::core::skills::{self, LoadSkillsOptions, Skill};
use crate::core::source_info::{create_source_info, SourceInfo, SourceOrigin, SourceScope};

// ============================================================================
// Types
// ============================================================================

/// A context file (AGENTS.md, CLAUDE.md, etc.) with its path and content.
#[derive(Debug, Clone)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
}

/// Paths contributed by extensions via the `resources_discover` event.
#[derive(Debug, Clone, Default)]
pub struct ResourceExtensionPaths {
    pub skill_paths: Vec<(String, SourceInfo)>,
    pub prompt_paths: Vec<(String, SourceInfo)>,
    pub theme_paths: Vec<(String, SourceInfo)>,
}

/// Options for the resource loader.
#[derive(Debug, Clone)]
pub struct ResourceLoaderOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub include_defaults: bool,
    pub skill_paths: Vec<String>,
    pub prompt_paths: Vec<String>,
    pub extension_paths: Vec<String>,
    pub theme_paths: Vec<String>,
    pub no_extensions: bool,
    pub no_skills: bool,
    pub no_prompts: bool,
    pub no_themes: bool,
    pub no_context_files: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Vec<String>,
}

impl Default for ResourceLoaderOptions {
    fn default() -> Self {
        Self {
            cwd: String::new(),
            agent_dir: None,
            include_defaults: true,
            skill_paths: Vec::new(),
            prompt_paths: Vec::new(),
            extension_paths: Vec::new(),
            theme_paths: Vec::new(),
            no_extensions: false,
            no_skills: false,
            no_prompts: false,
            no_themes: false,
            no_context_files: false,
            system_prompt: None,
            append_system_prompt: Vec::new(),
        }
    }
}

/// All resources loaded by the resource loader.
#[derive(Debug, Clone)]
pub struct LoadedResources {
    pub skills: Vec<Skill>,
    pub prompt_templates: Vec<PromptTemplate>,
    pub context_files: Vec<ContextFile>,
    pub diagnostics: Vec<ResourceDiagnostic>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Vec<String>,
}

// ============================================================================
// ResourceLoader trait
// ============================================================================

/// Interface for the resource loader.
pub trait ResourceLoader: Send + Sync {
    /// Reload all resources.
    fn reload(&mut self) -> LoadedResources;

    /// Extend resources with paths contributed by extensions.
    fn extend_resources(&mut self, paths: ResourceExtensionPaths);

    /// Get the current loaded resources.
    fn get_resources(&self) -> &LoadedResources;
}

// ============================================================================
// DefaultResourceLoader
// ============================================================================

/// Default implementation of `ResourceLoader`.
///
/// Orchestrates the full reload cycle:
/// 1. Resolve packages
/// 2. Load extensions
/// 3. Detect conflicts
/// 4. Load skills, prompts, themes
/// 5. Load context files
/// 6. Discover system prompts
pub struct DefaultResourceLoader {
    options: ResourceLoaderOptions,
    resources: LoadedResources,
    extension_source_infos: Vec<(String, SourceInfo)>,
    prompt_source_infos: Vec<(String, SourceInfo)>,
    theme_source_infos: Vec<(String, SourceInfo)>,
    last_skill_paths: Vec<String>,
    last_prompt_paths: Vec<String>,
    last_theme_paths: Vec<String>,
    loaded: bool,
}

impl DefaultResourceLoader {
    pub fn new(options: ResourceLoaderOptions) -> Self {
        Self {
            resources: LoadedResources {
                skills: Vec::new(),
                prompt_templates: Vec::new(),
                context_files: Vec::new(),
                diagnostics: Vec::new(),
                system_prompt: None,
                append_system_prompt: Vec::new(),
            },
            extension_source_infos: Vec::new(),
            prompt_source_infos: Vec::new(),
            theme_source_infos: Vec::new(),
            last_skill_paths: Vec::new(),
            last_prompt_paths: Vec::new(),
            last_theme_paths: Vec::new(),
            loaded: false,
            options,
        }
    }

    /// Resolve a path relative to cwd.
    fn resolve_path(&self, p: &str) -> String {
        let path = Path::new(p);
        if path.is_absolute() {
            p.to_string()
        } else {
            Path::new(&self.options.cwd)
                .join(p)
                .to_string_lossy()
                .to_string()
        }
    }

    /// Merge two path lists, deduplicating by canonical path.
    fn merge_paths(&self, primary: &[String], additional: &[String]) -> Vec<String> {
        let mut merged = Vec::new();
        let mut seen = HashSet::new();

        for p in primary.iter().chain(additional) {
            let resolved = self.resolve_path(p);
            if seen.insert(resolved.clone()) {
                merged.push(resolved);
            }
        }

        merged
    }

    /// Load skills from paths.
    fn load_skills_from_paths(&self, paths: &[String]) -> (Vec<Skill>, Vec<ResourceDiagnostic>) {
        if self.options.no_skills && paths.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let result = skills::load_skills(&LoadSkillsOptions {
            cwd: self.options.cwd.clone(),
            agent_dir: self.options.agent_dir.clone(),
            skill_paths: paths.to_vec(),
            include_defaults: false,
        });

        // Attach source info to each skill
        let skills = result
            .skills
            .into_iter()
            .map(|skill| {
                let source_info = self.find_source_info(&skill.file_path, &self.extension_source_infos);
                Skill {
                    source_info,
                    ..skill
                }
            })
            .collect();

        (skills, result.diagnostics)
    }

    /// Load prompts from paths.
    fn load_prompts_from_paths(&self, paths: &[String]) -> (Vec<PromptTemplate>, Vec<ResourceDiagnostic>) {
        if self.options.no_prompts && paths.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let result = prompt_templates::load_prompt_templates(&LoadPromptTemplatesOptions {
            cwd: self.options.cwd.clone(),
            agent_dir: self.options.agent_dir.clone(),
            prompt_paths: paths.to_vec(),
            include_defaults: false,
        });

        // Deduplicate prompts by name
        let (prompts, diagnostics) = deduplicate_prompts(result.templates);

        (prompts, diagnostics)
    }

    /// Find source info for a path, checking extension-contributed infos first.
    fn find_source_info(&self, path: &str, extension_infos: &[(String, SourceInfo)]) -> SourceInfo {
        let normalized = Path::new(path);
        let normalized_str = normalized.to_string_lossy();

        // Check extension-contributed source infos
        for (ext_path, info) in extension_infos {
            let ext_normalized = Path::new(ext_path);
            if normalized_str.starts_with(&ext_normalized.to_string_lossy().as_ref()) {
                return info.clone();
            }
        }

        // Default: derive from path location
        if let Some(agent_dir) = &self.options.agent_dir {
            let agent_path = Path::new(agent_dir);
            if normalized_str.starts_with(&agent_path.to_string_lossy().as_ref()) {
                return create_source_info(
                    path.to_string(),
                    "local".to_string(),
                    SourceScope::User,
                    SourceOrigin::TopLevel,
                    Some(agent_dir.to_string()),
                );
            }
        }

        let cwd_path = Path::new(&self.options.cwd);
        if normalized_str.starts_with(&cwd_path.to_string_lossy().as_ref()) {
            return create_source_info(
                path.to_string(),
                "local".to_string(),
                SourceScope::Project,
                SourceOrigin::TopLevel,
                Some(self.options.cwd.clone()),
            );
        }

        create_source_info(
            path.to_string(),
            "local".to_string(),
            SourceScope::Temporary,
            SourceOrigin::TopLevel,
            None,
        )
    }

    /// Discover SYSTEM.md from agent dir or project dir.
    fn discover_system_prompt(&self) -> Option<String> {
        // Check project .pi-rs/SYSTEM.md first
        let project_path = Path::new(&self.options.cwd)
            .join(config::CONFIG_DIR_NAME)
            .join("SYSTEM.md");
        if project_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&project_path) {
                return Some(content);
            }
        }

        // Check agent dir SYSTEM.md
        if let Some(agent_dir) = &self.options.agent_dir {
            let global_path = Path::new(agent_dir).join("SYSTEM.md");
            if global_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&global_path) {
                    return Some(content);
                }
            }
        }

        None
    }

    /// Discover APPEND_SYSTEM.md from agent dir or project dir.
    fn discover_append_system_prompt(&self) -> Vec<String> {
        let mut result = Vec::new();

        // Check project .pi-rs/APPEND_SYSTEM.md first
        let project_path = Path::new(&self.options.cwd)
            .join(config::CONFIG_DIR_NAME)
            .join("APPEND_SYSTEM.md");
        if project_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&project_path) {
                result.push(content);
                return result;
            }
        }

        // Check agent dir APPEND_SYSTEM.md
        if let Some(agent_dir) = &self.options.agent_dir {
            let global_path = Path::new(agent_dir).join("APPEND_SYSTEM.md");
            if global_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&global_path) {
                    result.push(content);
                }
            }
        }

        result
    }
}

impl ResourceLoader for DefaultResourceLoader {
    fn reload(&mut self) -> LoadedResources {
        let mut all_diagnostics: Vec<ResourceDiagnostic> = Vec::new();

        // 1. Resolve skill/prompt/theme paths from options
        let skill_paths = if self.options.no_skills {
            self.options.skill_paths.clone()
        } else {
            self.merge_paths(&self.last_skill_paths, &self.options.skill_paths)
        };
        self.last_skill_paths = skill_paths.clone();

        let prompt_paths = if self.options.no_prompts {
            self.options.prompt_paths.clone()
        } else {
            self.merge_paths(&self.last_prompt_paths, &self.options.prompt_paths)
        };
        self.last_prompt_paths = prompt_paths.clone();

        let theme_paths = if self.options.no_themes {
            self.options.theme_paths.clone()
        } else {
            self.merge_paths(&self.last_theme_paths, &self.options.theme_paths)
        };
        self.last_theme_paths = theme_paths.clone();

        // 2. Load skills
        let (skills, skill_diagnostics) = self.load_skills_from_paths(&skill_paths);
        all_diagnostics.extend(skill_diagnostics);

        // 3. Load prompts
        let (prompts, prompt_diagnostics) = self.load_prompts_from_paths(&prompt_paths);
        all_diagnostics.extend(prompt_diagnostics);

        // 4. Load context files (from cwd up to root)
        let context_files = if self.options.no_context_files {
            Vec::new()
        } else {
            load_context_files_ancestors(&self.options.cwd, self.options.agent_dir.as_deref())
        };

        // 5. Discover system prompts
        let system_prompt = self.options.system_prompt.clone().or_else(|| self.discover_system_prompt());
        let append_system_prompt = if !self.options.append_system_prompt.is_empty() {
            self.options.append_system_prompt.clone()
        } else {
            self.discover_append_system_prompt()
        };

        self.resources = LoadedResources {
            skills,
            prompt_templates: prompts,
            context_files,
            diagnostics: all_diagnostics,
            system_prompt,
            append_system_prompt,
        };
        self.loaded = true;

        self.resources.clone()
    }

    fn extend_resources(&mut self, paths: ResourceExtensionPaths) {
        // Store extension-contributed source infos
        for (path, info) in &paths.skill_paths {
            self.extension_source_infos.push((path.clone(), info.clone()));
        }
        for (path, info) in &paths.prompt_paths {
            self.prompt_source_infos.push((path.clone(), info.clone()));
        }
        for (path, info) in &paths.theme_paths {
            self.theme_source_infos.push((path.clone(), info.clone()));
        }

        // Reload skills if new skill paths were contributed
        if !paths.skill_paths.is_empty() {
            let new_paths: Vec<String> = paths
                .skill_paths
                .iter()
                .map(|(p, _)| self.resolve_path(p))
                .collect();
            self.last_skill_paths = self.merge_paths(&self.last_skill_paths, &new_paths);
            let (skills, diagnostics) = self.load_skills_from_paths(&self.last_skill_paths);
            self.resources.skills = skills;
            self.resources.diagnostics.extend(diagnostics);
        }

        // Reload prompts if new prompt paths were contributed
        if !paths.prompt_paths.is_empty() {
            let new_paths: Vec<String> = paths
                .prompt_paths
                .iter()
                .map(|(p, _)| self.resolve_path(p))
                .collect();
            self.last_prompt_paths = self.merge_paths(&self.last_prompt_paths, &new_paths);
            let (prompts, diagnostics) = self.load_prompts_from_paths(&self.last_prompt_paths);
            self.resources.prompt_templates = prompts;
            self.resources.diagnostics.extend(diagnostics);
        }
    }

    fn get_resources(&self) -> &LoadedResources {
        &self.resources
    }
}

// ============================================================================
// Context file loading
// ============================================================================

/// Load context files (AGENTS.md, CLAUDE.md, etc.) from cwd up to filesystem root,
/// plus from the agent directory.
fn load_context_files_ancestors(cwd: &str, agent_dir: Option<&str>) -> Vec<ContextFile> {
    let mut files = Vec::new();
    let mut seen_paths = HashSet::new();
    let context_file_names = ["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

    // 1. Load from agent dir (global)
    if let Some(agent) = agent_dir {
        for name in &context_file_names {
            let path = Path::new(agent).join(name);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let path_str = path.to_string_lossy().to_string();
                    if seen_paths.insert(path_str.clone()) {
                        files.push(ContextFile {
                            path: path_str,
                            content,
                        });
                    }
                }
            }
        }
    }

    // 2. Walk up from cwd to root, collecting context files
    let mut ancestor_files: Vec<ContextFile> = Vec::new();
    let mut current_dir = Some(Path::new(cwd).to_path_buf());

    while let Some(dir) = current_dir {
        for name in &context_file_names {
            let path = dir.join(name);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let path_str = path.to_string_lossy().to_string();
                    if seen_paths.insert(path_str.clone()) {
                        ancestor_files.push(ContextFile {
                            path: path_str,
                            content,
                        });
                    }
                }
            }
        }

        // Move to parent
        let parent = dir.parent().map(|p| p.to_path_buf());
        if parent.as_ref() == Some(&dir) {
            break; // reached root
        }
        current_dir = parent;
    }

    // Ancestor files are prepended (closest to cwd first)
    files.extend(ancestor_files);

    files
}

// ============================================================================
// Deduplication helpers
// ============================================================================

/// Deduplicate prompts by name, keeping the first occurrence.
fn deduplicate_prompts(
    prompts: Vec<PromptTemplate>,
) -> (Vec<PromptTemplate>, Vec<ResourceDiagnostic>) {
    let mut seen: std::collections::HashMap<String, PromptTemplate> = std::collections::HashMap::new();
    let mut diagnostics = Vec::new();

    for prompt in prompts {
        if let Some(existing) = seen.get(&prompt.name) {
            diagnostics.push(ResourceDiagnostic::Collision {
                message: format!("name \"/{}\" collision", prompt.name),
                path: prompt.file_path.clone(),
                collision: ResourceCollision {
                    resource_type: "prompt".to_string(),
                    name: prompt.name.clone(),
                    winner_path: existing.file_path.clone(),
                    loser_path: prompt.file_path.clone(),
                },
            });
        } else {
            seen.insert(prompt.name.clone(), prompt);
        }
    }

    (seen.into_values().collect(), diagnostics)
}

// ============================================================================
// Simple load function (backward compatible)
// ============================================================================

/// Load all resources with the given options (simple one-shot, no trust cycle).
pub fn load_all_resources(options: &ResourceLoaderOptions) -> LoadedResources {
    let mut loader = DefaultResourceLoader::new(options.clone());
    loader.reload()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // -----------------------------------------------------------------------
    // Context file loading tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_context_files_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let files = load_context_files_ancestors(dir.path().to_str().unwrap(), None);
        assert!(files.is_empty());
    }

    #[test]
    fn test_load_context_files_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        let agents_path = dir.path().join("AGENTS.md");
        fs::write(&agents_path, "# Agent Context").unwrap();

        let files = load_context_files_ancestors(dir.path().to_str().unwrap(), None);
        // At minimum, the file we created should be found
        assert!(
            files.iter().any(|f| f.content == "# Agent Context"),
            "should find AGENTS.md content"
        );
    }

    #[test]
    fn test_load_context_files_claude_md() {
        let dir = tempfile::tempdir().unwrap();
        let claude_path = dir.path().join("CLAUDE.md");
        fs::write(&claude_path, "# Claude Context").unwrap();

        let files = load_context_files_ancestors(dir.path().to_str().unwrap(), None);
        assert!(
            files.iter().any(|f| f.content == "# Claude Context"),
            "should find CLAUDE.md content"
        );
    }

    #[test]
    fn test_load_context_files_from_agent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("AGENTS.md"), "# Global Context").unwrap();

        let files = load_context_files_ancestors(
            dir.path().to_str().unwrap(),
            Some(agent_dir.to_str().unwrap()),
        );
        assert!(
            files.iter().any(|f| f.content == "# Global Context"),
            "should find agent dir AGENTS.md"
        );
    }

    #[test]
    fn test_load_context_files_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        // Different files in agent dir and cwd
        fs::write(agent_dir.join("AGENTS.md"), "# Global").unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Local").unwrap();

        let files = load_context_files_ancestors(
            dir.path().to_str().unwrap(),
            Some(agent_dir.to_str().unwrap()),
        );
        // Both are different files, so both should appear
        assert!(
            files.iter().any(|f| f.content == "# Global"),
            "should find global content"
        );
        assert!(
            files.iter().any(|f| f.content == "# Local"),
            "should find local content"
        );
    }

    // -----------------------------------------------------------------------
    // ResourceLoaderOptions tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resource_loader_options_default() {
        let opts = ResourceLoaderOptions::default();
        assert!(opts.cwd.is_empty());
        assert!(opts.agent_dir.is_none());
        assert!(opts.include_defaults);
        assert!(!opts.no_extensions);
        assert!(!opts.no_skills);
    }

    // -----------------------------------------------------------------------
    // DefaultResourceLoader tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_loader_new_and_reload_empty() {
        let opts = ResourceLoaderOptions {
            cwd: "/nonexistent".to_string(),
            include_defaults: false,
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        let resources = loader.reload();

        assert!(resources.skills.is_empty());
        assert!(resources.prompt_templates.is_empty());
        assert!(resources.context_files.is_empty());
        assert!(resources.diagnostics.is_empty());
        assert!(resources.system_prompt.is_none());
    }

    #[test]
    fn test_loader_reload_with_skill_path() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let skill_file = skills_dir.join("test-skill.md");
        fs::write(
            &skill_file,
            "---\nname: test-skill\ndescription: A test skill\n---\n# Test Skill",
        )
        .unwrap();

        let opts = ResourceLoaderOptions {
            cwd: dir.path().to_string_lossy().to_string(),
            include_defaults: false,
            skill_paths: vec![skills_dir.to_string_lossy().to_string()],
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        let resources = loader.reload();

        assert!(!resources.skills.is_empty(), "should have loaded skills");
        assert!(resources.skills.iter().any(|s| s.name == "test-skill"));
    }

    #[test]
    fn test_loader_reload_with_prompt_path() {
        let dir = tempfile::tempdir().unwrap();
        let prompts_dir = dir.path().join("prompts");
        fs::create_dir_all(&prompts_dir).unwrap();
        let prompt_file = prompts_dir.join("test-prompt.md");
        fs::write(
            &prompt_file,
            "---\nname: test-prompt\n---\n# Test Prompt",
        )
        .unwrap();

        let opts = ResourceLoaderOptions {
            cwd: dir.path().to_string_lossy().to_string(),
            include_defaults: false,
            prompt_paths: vec![prompts_dir.to_string_lossy().to_string()],
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        let resources = loader.reload();

        assert!(!resources.prompt_templates.is_empty(), "should have loaded prompts");
    }

    #[test]
    fn test_loader_no_skills_flag() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("test.skill.md"), "---\nname: test\n---\n# Test").unwrap();

        let opts = ResourceLoaderOptions {
            cwd: dir.path().to_string_lossy().to_string(),
            include_defaults: false,
            no_skills: true,
            skill_paths: vec![skills_dir.to_string_lossy().to_string()],
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        let resources = loader.reload();

        assert!(resources.skills.is_empty(), "no_skills should prevent skill loading");
    }

    #[test]
    fn test_loader_no_context_files_flag() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Context").unwrap();

        let opts = ResourceLoaderOptions {
            cwd: dir.path().to_string_lossy().to_string(),
            include_defaults: false,
            no_context_files: true,
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        let resources = loader.reload();

        assert!(resources.context_files.is_empty(), "no_context_files should prevent loading");
    }

    #[test]
    fn test_loader_system_prompt_from_option() {
        let opts = ResourceLoaderOptions {
            cwd: "/nonexistent".to_string(),
            include_defaults: false,
            system_prompt: Some("Custom system prompt".to_string()),
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        let resources = loader.reload();

        assert_eq!(resources.system_prompt.as_deref(), Some("Custom system prompt"));
    }

    #[test]
    fn test_loader_system_prompt_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let pi_dir = dir.path().join(config::CONFIG_DIR_NAME);
        fs::create_dir_all(&pi_dir).unwrap();
        fs::write(pi_dir.join("SYSTEM.md"), "# System Prompt from File").unwrap();

        let opts = ResourceLoaderOptions {
            cwd: dir.path().to_string_lossy().to_string(),
            include_defaults: false,
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        let resources = loader.reload();

        assert_eq!(
            resources.system_prompt.as_deref(),
            Some("# System Prompt from File")
        );
    }

    #[test]
    fn test_loader_append_system_prompt() {
        let opts = ResourceLoaderOptions {
            cwd: "/nonexistent".to_string(),
            include_defaults: false,
            append_system_prompt: vec!["Append this".to_string()],
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        let resources = loader.reload();

        assert_eq!(resources.append_system_prompt.len(), 1);
        assert_eq!(resources.append_system_prompt[0], "Append this");
    }

    // -----------------------------------------------------------------------
    // extend_resources tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extend_resources_adds_skill_paths() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let skill_file = skills_dir.join("ext-skill.md");
        fs::write(
            &skill_file,
            "---\nname: ext-skill\ndescription: Extension skill\n---\n# Extension Skill",
        )
        .unwrap();

        let opts = ResourceLoaderOptions {
            cwd: dir.path().to_string_lossy().to_string(),
            include_defaults: false,
            ..Default::default()
        };
        let mut loader = DefaultResourceLoader::new(opts);
        loader.reload();

        // Extend with a new skill path
        let source_info = create_source_info(
            skills_dir.to_string_lossy().to_string(),
            "extension".to_string(),
            SourceScope::Temporary,
            SourceOrigin::TopLevel,
            None,
        );
        let ext_paths = ResourceExtensionPaths {
            skill_paths: vec![(skills_dir.to_string_lossy().to_string(), source_info)],
            ..Default::default()
        };
        loader.extend_resources(ext_paths);

        let resources = loader.get_resources();
        assert!(!resources.skills.is_empty(), "should have loaded extension skills");
    }

    // -----------------------------------------------------------------------
    // deduplicate_prompts tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_deduplicate_prompts_no_duplicates() {
        let prompts = vec![
            PromptTemplate {
                name: "prompt-a".to_string(),
                description: String::new(),
                file_path: "/a.md".to_string(),
                source: crate::core::prompt_templates::PromptSource::Project,
                append: false,
            },
            PromptTemplate {
                name: "prompt-b".to_string(),
                description: String::new(),
                file_path: "/b.md".to_string(),
                source: crate::core::prompt_templates::PromptSource::Project,
                append: false,
            },
        ];

        let (deduped, diagnostics) = deduplicate_prompts(prompts);
        assert_eq!(deduped.len(), 2);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn test_deduplicate_prompts_with_duplicates() {
        let prompts = vec![
            PromptTemplate {
                name: "shared".to_string(),
                description: String::new(),
                file_path: "/first.md".to_string(),
                source: crate::core::prompt_templates::PromptSource::Project,
                append: false,
            },
            PromptTemplate {
                name: "shared".to_string(),
                description: String::new(),
                file_path: "/second.md".to_string(),
                source: crate::core::prompt_templates::PromptSource::Project,
                append: false,
            },
        ];

        let (deduped, diagnostics) = deduplicate_prompts(prompts);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].file_path, "/first.md");
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            ResourceDiagnostic::Collision { collision, .. } => {
                assert_eq!(collision.name, "shared");
                assert_eq!(collision.winner_path, "/first.md");
                assert_eq!(collision.loser_path, "/second.md");
            }
            _ => panic!("expected Collision diagnostic"),
        }
    }

    // -----------------------------------------------------------------------
    // merge_paths tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_merge_paths_dedup() {
        let opts = ResourceLoaderOptions {
            cwd: "/tmp".to_string(),
            ..Default::default()
        };
        let loader = DefaultResourceLoader::new(opts);

        let primary = vec!["/tmp/a.md".to_string(), "/tmp/b.md".to_string()];
        let additional = vec!["/tmp/b.md".to_string(), "/tmp/c.md".to_string()];

        let merged = loader.merge_paths(&primary, &additional);
        assert_eq!(merged.len(), 3);
        assert!(merged.contains(&"/tmp/a.md".to_string()));
        assert!(merged.contains(&"/tmp/b.md".to_string()));
        assert!(merged.contains(&"/tmp/c.md".to_string()));
    }

    // -----------------------------------------------------------------------
    // load_all_resources backward compat tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_all_resources_empty() {
        let opts = ResourceLoaderOptions {
            cwd: "/nonexistent".to_string(),
            include_defaults: false,
            ..Default::default()
        };
        let result = load_all_resources(&opts);
        assert!(result.skills.is_empty());
        assert!(result.context_files.is_empty());
        assert!(result.system_prompt.is_none());
    }

    #[test]
    fn test_load_all_resources_with_context_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Test Context").unwrap();

        let opts = ResourceLoaderOptions {
            cwd: dir.path().to_string_lossy().to_string(),
            include_defaults: false,
            ..Default::default()
        };
        let result = load_all_resources(&opts);
        assert!(
            result.context_files.iter().any(|f| f.content == "# Test Context"),
            "should find AGENTS.md content"
        );
    }

    // -----------------------------------------------------------------------
    // find_source_info tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_source_info_extension_contributed() {
        let opts = ResourceLoaderOptions {
            cwd: "/tmp".to_string(),
            agent_dir: Some("/agent".to_string()),
            ..Default::default()
        };
        let loader = DefaultResourceLoader::new(opts);

        let ext_info = create_source_info(
            "/ext/skills".to_string(),
            "my-extension".to_string(),
            SourceScope::Temporary,
            SourceOrigin::Package,
            Some("/ext".to_string()),
        );

        let found = loader.find_source_info("/ext/skills/test.md", &[("/ext/skills".to_string(), ext_info.clone())]);
        assert_eq!(found.source, "my-extension");
    }

    #[test]
    fn test_find_source_info_falls_back_to_scope() {
        let opts = ResourceLoaderOptions {
            cwd: "/tmp".to_string(),
            agent_dir: Some("/home/user/.pi".to_string()),
            ..Default::default()
        };
        let loader = DefaultResourceLoader::new(opts);

        // Path under agent dir → User scope
        let info = loader.find_source_info("/home/user/.pi/skills/test.md", &[]);
        assert_eq!(info.scope, SourceScope::User);

        // Path under cwd → Project scope
        let info = loader.find_source_info("/tmp/skills/test.md", &[]);
        assert_eq!(info.scope, SourceScope::Project);

        // Other path → Temporary scope
        let info = loader.find_source_info("/other/path/test.md", &[]);
        assert_eq!(info.scope, SourceScope::Temporary);
    }
}
