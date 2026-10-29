use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::core::diagnostics::ResourceDiagnostic;
use crate::core::source_info::{create_source_info, SourceInfo, SourceOrigin, SourceScope};
use crate::utils::paths::{normalize_path, PathOptions};

// ============================================================================
// Constants
// ============================================================================

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: &[&str] = &[".gitignore", ".ignore", ".fdignore"];

// ============================================================================
// Types
// ============================================================================

/// Frontmatter fields for a skill file, matching the original TypeScript SkillFrontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "disable-model-invocation")]
    pub disable_model_invocation: Option<bool>,
}

/// A loaded skill, matching the original TypeScript Skill interface.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub base_dir: String,
    pub source_info: SourceInfo,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LoadSkillsOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub skill_paths: Vec<String>,
    pub include_defaults: bool,
    /// Override for `$HOME` (used to locate the user-level `.agents/skills`).
    /// `None` = `dirs::home_dir()`. Tests set this to keep the host's
    /// `~/.agents/skills` out of the fixture.
    pub home_dir: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

// ============================================================================
// Validation
// ============================================================================

/// Validate skill name per Agent Skills spec.
fn validate_name(name: &str) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    if name.len() > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {} characters ({})",
            MAX_NAME_LENGTH,
            name.len()
        ));
    }

    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)".to_string(),
        );
    }

    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }

    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }

    errors
}

/// Validate description per Agent Skills spec.
fn validate_description(description: Option<&str>) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    match description {
        None | Some("") => {
            errors.push("description is required".to_string());
        }
        Some(d) if d.trim().is_empty() => {
            errors.push("description is required".to_string());
        }
        Some(d) if d.len() > MAX_DESCRIPTION_LENGTH => {
            errors.push(format!(
                "description exceeds {} characters ({})",
                MAX_DESCRIPTION_LENGTH,
                d.len()
            ));
        }
        Some(_) => {}
    }

    errors
}

// ============================================================================
// Frontmatter parsing
// ============================================================================

/// Parse YAML-like frontmatter from markdown text.
/// Frontmatter is delimited by `---` at the start of the file.
fn parse_frontmatter(content: &str) -> (SkillFrontmatter, String, bool) {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return (
            SkillFrontmatter {
                name: None,
                description: None,
                disable_model_invocation: None,
            },
            normalized,
            false,
        );
    }

    // Find the closing ---
    let Some(end_marker) = normalized[3..].find("\n---") else {
        return (
            SkillFrontmatter {
                name: None,
                description: None,
                disable_model_invocation: None,
            },
            normalized,
            true,
        );
    };

    let end_idx = end_marker + 3;
    let yaml_str = &normalized[4..end_idx];
    let body = normalized[end_idx + 4..].trim().to_string();

    // Use serde_yaml for full YAML parsing (matching original TS `parse` from "yaml")
    let frontmatter: SkillFrontmatter = match serde_yaml::from_str(yaml_str) {
        Ok(fm) => fm,
        Err(_) => {
            // Return a special marker: description = Some("") signals parse failure
            // so the caller can emit a diagnostic
            return (
                SkillFrontmatter {
                    name: None,
                    description: Some(String::new()),
                    disable_model_invocation: None,
                },
                body,
                true,
            );
        }
    };

    (
        frontmatter,
        body,
        true,
    )
}

// ============================================================================
// Source info creation
// ============================================================================

pub fn create_skill_source_info(file_path: &str, base_dir: &str, source: &str) -> SourceInfo {
    let scope = match source {
        "user" => SourceScope::User,
        "project" => SourceScope::Project,
        _ => SourceScope::Temporary,
    };
    create_source_info(
        file_path.to_string(),
        "local".to_string(),
        scope,
        SourceOrigin::TopLevel,
        Some(base_dir.to_string()),
    )
}

// ============================================================================
// Loading
// ============================================================================

/// Load a single skill from a markdown file.
fn load_skill_from_file(
    file_path: &str,
    source: &str,
) -> (Option<Skill>, Vec<ResourceDiagnostic>) {
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => {
            diagnostics.push(ResourceDiagnostic::Warning {
                message: format!("failed to read skill file: {}", e),
                path: file_path.to_string(),
            });
            return (None, diagnostics);
        }
    };

    let skill_dir = Path::new(file_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent_dir_name = Path::new(file_path)
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let (frontmatter, _body, has_frontmatter) = parse_frontmatter(&content);

    // Check for YAML parse failure (description = Some("") signals this)
    if has_frontmatter && frontmatter.description.as_deref() == Some("") {
        diagnostics.push(ResourceDiagnostic::Warning {
            message: "failed to parse YAML frontmatter".to_string(),
            path: file_path.to_string(),
        });
        return (None, diagnostics);
    }

    // Validate description
    let desc_errors = validate_description(frontmatter.description.as_deref());
    for error in &desc_errors {
        diagnostics.push(ResourceDiagnostic::Warning {
            message: error.clone(),
            path: file_path.to_string(),
        });
    }

    // Use name from frontmatter, or fall back to parent directory name
    let name = frontmatter
        .name
        .clone()
        .unwrap_or(parent_dir_name);

    // Validate name
    let name_errors = validate_name(&name);
    for error in &name_errors {
        diagnostics.push(ResourceDiagnostic::Warning {
            message: error.clone(),
            path: file_path.to_string(),
        });
    }

    // Still load the skill even with warnings (unless description is completely missing)
    let description = match frontmatter.description {
        Some(ref d) if !d.trim().is_empty() => d.clone(),
        _ => {
            return (None, diagnostics);
        }
    };

    let skill = Skill {
        name,
        description,
        file_path: file_path.to_string(),
        base_dir: skill_dir.clone(),
        source_info: create_skill_source_info(file_path, &skill_dir, source),
        disable_model_invocation: frontmatter.disable_model_invocation.unwrap_or(false),
    };

    (Some(skill), diagnostics)
}

/// A simple ignore matcher for gitignore-style patterns.
#[derive(Debug, Clone, Default)]
struct IgnoreMatcher {
    patterns: Vec<String>,
}

impl IgnoreMatcher {
    fn new() -> Self {
        Self::default()
    }

    fn add_patterns(&mut self, patterns: &[String]) {
        self.patterns.extend(patterns.iter().cloned());
    }

    fn ignores(&self, path: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        for pattern in &self.patterns {
            if self.match_pattern(path, pattern) {
                return true;
            }
        }
        false
    }

    fn match_pattern(&self, path: &str, pattern: &str) -> bool {
        let trimmed = pattern.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return false;
        }
        let pat = trimmed.strip_prefix('/').unwrap_or(trimmed);
        if pat.contains('*') {
            let suffix = pat.trim_start_matches('*');
            if pat.starts_with('*') && pat.ends_with('*') {
                return path.contains(suffix);
            } else if pat.ends_with('*') {
                return path.starts_with(pat.trim_end_matches('*'));
            } else if pat.starts_with('*') {
                return path.ends_with(suffix);
            }
        }
        path == pat || path.ends_with(&format!("/{}", pat))
    }
}

/// Load ignore rules from .gitignore/.ignore/.fdignore files in a directory.
fn load_ignore_rules(dir: &Path) -> IgnoreMatcher {
    let mut matcher = IgnoreMatcher::new();
    for filename in IGNORE_FILE_NAMES {
        let ignore_path = dir.join(filename);
        if !ignore_path.exists() || !ignore_path.is_file() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&ignore_path) {
            let patterns: Vec<String> = content
                .lines()
                .filter_map(|line| {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || (trimmed.starts_with('#') && !trimmed.starts_with("\\#")) {
                        return None;
                    }
                    let pattern = trimmed.to_string();
                    Some(pattern)
                })
                .collect();
            if !patterns.is_empty() {
                matcher.add_patterns(&patterns);
            }
        }
    }
    matcher
}

#[derive(Clone, Copy)]
enum SkillDiscoveryMode {
    /// `.pi-rs`/user directories: root Markdown files are skills, nested
    /// Markdown files are only discovered when they are `SKILL.md`.
    Pi,
    /// `.agents/skills`: root Markdown files are ignored, nested Markdown
    /// files are skills.
    Agents,
}

/// Load skills from a directory, with recursive discovery.
///
/// Discovery rules:
/// - if a directory contains SKILL.md, treat it as a skill root and do not recurse further
/// - otherwise, load direct .md children in the root
/// - recurse into subdirectories to find SKILL.md
/// - honors .gitignore/.ignore/.fdignore files
fn load_skills_from_dir(
    dir: &Path,
    source: &str,
    mode: SkillDiscoveryMode,
) -> (Vec<Skill>, Vec<ResourceDiagnostic>) {
    load_skills_from_dir_internal(dir, source, mode, true)
}

fn load_skills_from_dir_internal(
    dir: &Path,
    source: &str,
    mode: SkillDiscoveryMode,
    is_root: bool,
) -> (Vec<Skill>, Vec<ResourceDiagnostic>) {
    let mut skills: Vec<Skill> = Vec::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    if !dir.exists() {
        return (skills, diagnostics);
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            diagnostics.push(ResourceDiagnostic::Warning {
                message: format!("failed to read skills directory: {}", e),
                path: dir.to_string_lossy().to_string(),
            });
            return (skills, diagnostics);
        }
    };

    let ignore_matcher = load_ignore_rules(dir);

    let entries: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_name() != "node_modules")
        .collect();

    // First pass: look for SKILL.md
    for entry in &entries {
        if entry.file_name() != "SKILL.md" {
            continue;
        }

        let full_path = entry.path();
        if !full_path.is_file() && !full_path.is_symlink() {
            continue;
        }

        // Check ignore rules
        let rel_path = full_path.strip_prefix(dir).unwrap_or(&full_path);
        if ignore_matcher.ignores(&rel_path.to_string_lossy()) {
            continue;
        }

        let path_str = full_path.to_string_lossy().to_string();
        let (skill, diags) = load_skill_from_file(&path_str, source);
        diagnostics.extend(diags);
        if let Some(s) = skill {
            skills.push(s);
        }
        return (skills, diagnostics);
    }

    // Second pass: recurse into subdirectories and load .md files
    for entry in entries {
        let full_path = entry.path();

        // Check ignore rules
        let rel_path = full_path.strip_prefix(dir).unwrap_or(&full_path);
        let rel_str = rel_path.to_string_lossy();

        if full_path.is_dir() {
            // For directories, check with trailing slash
            if ignore_matcher.ignores(&format!("{}/", rel_str)) {
                continue;
            }
            let (sub_skills, sub_diags) =
                load_skills_from_dir_internal(&full_path, source, mode, false);
            skills.extend(sub_skills);
            diagnostics.extend(sub_diags);
        } else if (full_path.is_file() || full_path.is_symlink())
            && match mode {
                SkillDiscoveryMode::Pi => is_root,
                SkillDiscoveryMode::Agents => !is_root,
            }
        {
            if ignore_matcher.ignores(&rel_str) {
                continue;
            }
            if full_path.extension().map(|e| e == "md").unwrap_or(false) {
                let path_str = full_path.to_string_lossy().to_string();
                let (skill, diags) = load_skill_from_file(&path_str, source);
                diagnostics.extend(diags);
                if let Some(s) = skill {
                    skills.push(s);
                }
            }
        }
    }

    (skills, diagnostics)
}

// ============================================================================
// Public API
// ============================================================================

/// The user-level `.agents/skills` directory (`$HOME/.agents/skills`).
///
/// `home_dir` overrides `$HOME` (tests); `None` falls back to `dirs::home_dir()`.
pub fn home_agents_skills_dir(home_dir: Option<&str>) -> Option<String> {
    let home = home_dir
        .map(|h| h.to_string())
        .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))?;
    let home = resolve_absolute_path(&home, None, false, Some(&home), false);
    Some(
        Path::new(&home)
            .join(".agents")
            .join("skills")
            .to_string_lossy()
            .to_string(),
    )
}

/// `.agents/skills` directories from `cwd` upward through the git repository
/// root (inclusive). Mirrors TS `collectAncestorAgentsSkillDirs`: the walk stops
/// *after* the directory that contains `.git`, or at the filesystem root when
/// `cwd` is not inside a repository.
pub fn collect_ancestor_agents_skill_dirs(cwd: &str) -> Vec<String> {
    // Node's `path.resolve` is lexical: it must not follow symlinks while
    // deciding where the repository boundary is.
    let start = PathBuf::from(resolve_absolute_path(cwd, None, false, None, false));
    let git_root = find_git_repo_root(&start);
    let mut dirs: Vec<String> = Vec::new();
    let mut current = start;

    loop {
        dirs.push(
            Path::new(&current)
                .join(".agents")
                .join("skills")
                .to_string_lossy()
                .to_string(),
        );
        if git_root.as_deref() == Some(current.as_path()) {
            break;
        }
        let parent = match Path::new(&current).parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_string_lossy().to_string(),
            _ => break,
        };
        if parent == current {
            break;
        }
        current = PathBuf::from(parent);
    }

    dirs
}

/// Nearest ancestor of `start` (inclusive) that contains a `.git` entry.
fn find_git_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Resolve like Node's `path.resolve`: expand `~`, make the path absolute,
/// normalize `.`/`..` lexically, and never follow symlinks.
fn resolve_absolute_path(
    input: &str,
    base_dir: Option<&str>,
    trim: bool,
    home_dir: Option<&str>,
    expand_tilde: bool,
) -> String {
    let options = PathOptions {
        trim,
        expand_tilde,
        home_dir: home_dir.map(str::to_string),
        ..PathOptions::default()
    };
    let normalized = normalize_path(input, &options);
    let path = Path::new(&normalized);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let base = base_dir
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        base.join(path)
    };
    normalize_lexical_components(&absolute)
        .to_string_lossy()
        .to_string()
}

fn normalize_lexical_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.has_root() && normalized.parent().is_none() {
                    continue;
                }
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn paths_equal_lexically(left: &str, right: &str) -> bool {
    resolve_absolute_path(left, None, false, None, false)
        == resolve_absolute_path(right, None, false, None, false)
}

fn rebase_skill_source_info(
    mut result: (Vec<Skill>, Vec<ResourceDiagnostic>),
    base_dir: &Path,
    source: &str,
) -> (Vec<Skill>, Vec<ResourceDiagnostic>) {
    for skill in &mut result.0 {
        skill.source_info = create_skill_source_info(
            &skill.file_path,
            &base_dir.to_string_lossy(),
            source,
        );
    }
    result
}

/// Load skills from all configured locations.
///
/// Discovery order matches TS `PackageManager.addAutoDiscoveredResources` +
/// `loadSkills` (first load wins on a name collision, and the resulting order
/// is the system-prompt order):
///
/// 1. project `{cwd}/.pi-rs/skills`
/// 2. project `.agents/skills` from `{cwd}` up to the git repo root
/// 3. user `{agent_dir}/skills`
/// 4. user `~/.agents/skills`
/// 5. explicit `skill_paths` (extension / `--skills`)
pub fn load_skills(options: &LoadSkillsOptions) -> LoadSkillsResult {
    let resolved_agent_dir = options
        .agent_dir
        .as_deref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| config::get_agent_dir().to_string_lossy().to_string());

    // Insertion-ordered: `HashMap` iteration order would make both the
    // collision winner and the `<available_skills>` listing nondeterministic.
    let mut ordered: Vec<Skill> = Vec::new();
    let mut name_index: HashMap<String, usize> = HashMap::new();
    let mut real_path_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut all_diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    fn add_skills(
        ordered: &mut Vec<Skill>,
        name_index: &mut HashMap<String, usize>,
        real_path_set: &mut std::collections::HashSet<String>,
        diagnostics: &mut Vec<ResourceDiagnostic>,
        result: (Vec<Skill>, Vec<ResourceDiagnostic>),
    ) {
        let (new_skills, diags) = result;
        diagnostics.extend(diags);
        for skill in new_skills {
            // Resolve symlinks to detect duplicate files
            let real_path = std::fs::canonicalize(&skill.file_path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| skill.file_path.clone());

            // Skip silently if we've already loaded this exact file (via symlink)
            if real_path_set.contains(&real_path) {
                continue;
            }

            if let Some(&idx) = name_index.get(&skill.name) {
                let winner_path = ordered[idx].file_path.clone();
                diagnostics.push(ResourceDiagnostic::Collision {
                    message: format!("name \"{}\" collision", skill.name),
                    path: skill.file_path.clone(),
                    collision: crate::core::diagnostics::ResourceCollision {
                        resource_type: "skill".to_string(),
                        name: skill.name.clone(),
                        winner_path,
                        loser_path: skill.file_path.clone(),
                    },
                });
            } else {
                name_index.insert(skill.name.clone(), ordered.len());
                ordered.push(skill);
                real_path_set.insert(real_path);
            }
        }
    }

    if options.include_defaults {
        // Project resources first: TS adds them before user resources, so a
        // project skill wins a name collision against a user skill.
        let project_skills_dir = Path::new(&options.cwd)
            .join(config::CONFIG_DIR_NAME)
            .join("skills");
        add_skills(
            &mut ordered,
            &mut name_index,
            &mut real_path_set,
            &mut all_diagnostics,
            rebase_skill_source_info(
                load_skills_from_dir(&project_skills_dir, "project", SkillDiscoveryMode::Pi),
                project_skills_dir
                    .parent()
                    .unwrap_or(Path::new(&options.cwd)),
                "project",
            ),
        );

        // `.agents/skills` in every ancestor up to the git repo root, skipping
        // the user-level one (it is loaded below as a user resource).
        let home_agents_dir = home_agents_skills_dir(options.home_dir.as_deref());
        for dir in collect_ancestor_agents_skill_dirs(&options.cwd) {
            if let Some(ref global) = home_agents_dir {
                if paths_equal_lexically(&dir, global) {
                    continue;
                }
            }
            let agents_dir = Path::new(&dir);
            let source_base_dir = agents_dir.parent().unwrap_or(agents_dir);
            add_skills(
                &mut ordered,
                &mut name_index,
                &mut real_path_set,
                &mut all_diagnostics,
                rebase_skill_source_info(
                    // `.agents` discovery excludes root-level Markdown files;
                    // nested Markdown files and `SKILL.md` entries qualify.
                    load_skills_from_dir(agents_dir, "project", SkillDiscoveryMode::Agents),
                    source_base_dir,
                    "project",
                ),
            );
        }

        let user_skills_dir = Path::new(&resolved_agent_dir).join("skills");
        add_skills(
            &mut ordered,
            &mut name_index,
            &mut real_path_set,
            &mut all_diagnostics,
            rebase_skill_source_info(
                load_skills_from_dir(&user_skills_dir, "user", SkillDiscoveryMode::Pi),
                Path::new(&resolved_agent_dir),
                "user",
            ),
        );

        if let Some(dir) = home_agents_dir {
            let user_agents_dir = Path::new(&dir);
            let source_base_dir = user_agents_dir.parent().unwrap_or(user_agents_dir);
            add_skills(
                &mut ordered,
                &mut name_index,
                &mut real_path_set,
                &mut all_diagnostics,
                rebase_skill_source_info(
                    load_skills_from_dir(user_agents_dir, "user", SkillDiscoveryMode::Agents),
                    source_base_dir,
                    "user",
                ),
            );
        }
    }

    for raw_path in &options.skill_paths {
        let resolved_path = resolve_absolute_path(
            raw_path,
            Some(&options.cwd),
            true,
            options.home_dir.as_deref(),
            true,
        );
        let path = PathBuf::from(&resolved_path);
        if !path.exists() {
            all_diagnostics.push(ResourceDiagnostic::Warning {
                message: "skill path does not exist".to_string(),
                path: resolved_path,
            });
            continue;
        }

        if path.is_dir() {
            add_skills(
                &mut ordered,
                &mut name_index,
                &mut real_path_set,
                &mut all_diagnostics,
                load_skills_from_dir(&path, "path", SkillDiscoveryMode::Pi),
            );
        } else if path.is_file() && resolved_path.ends_with(".md") {
            let (skill, diags) = load_skill_from_file(&resolved_path, "path");
            all_diagnostics.extend(diags);
            if let Some(s) = skill {
                add_skills(
                    &mut ordered,
                    &mut name_index,
                    &mut real_path_set,
                    &mut all_diagnostics,
                    (vec![s], vec![]),
                );
            }
        } else if path.is_file() {
            all_diagnostics.push(ResourceDiagnostic::Warning {
                message: "skill path is not a markdown file".to_string(),
                path: resolved_path,
            });
        }
    }

    LoadSkillsResult {
        skills: ordered,
        diagnostics: all_diagnostics,
    }
}

// ============================================================================
// Prompt formatting
// ============================================================================

/// Format skills for inclusion in a system prompt.
/// Uses XML format per Agent Skills standard.
///
/// Skills with disableModelInvocation=true are excluded from the prompt
/// (they can only be invoked explicitly via /skill:name commands).
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();

    if visible.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description.".to_string(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in &visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path)
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_validate_name_valid() {
        let errors = validate_name("my-skill");
        assert!(errors.is_empty());
    }

    #[test]
    fn test_validate_name_too_long() {
        let long_name = "a".repeat(MAX_NAME_LENGTH + 1);
        let errors = validate_name(&long_name);
        assert!(errors.iter().any(|e| e.contains("exceeds")));
    }

    #[test]
    fn test_validate_name_invalid_chars() {
        let errors = validate_name("My Skill!");
        assert!(errors.iter().any(|e| e.contains("invalid characters")));
    }

    #[test]
    fn test_validate_name_hyphen_start() {
        let errors = validate_name("-skill");
        assert!(errors.iter().any(|e| e.contains("hyphen")));
    }

    #[test]
    fn test_validate_name_consecutive_hyphens() {
        let errors = validate_name("my--skill");
        assert!(errors.iter().any(|e| e.contains("consecutive hyphens")));
    }

    #[test]
    fn test_validate_description_valid() {
        let errors = validate_description(Some("A valid description"));
        assert!(errors.is_empty());
    }

    #[test]
    fn test_validate_description_empty() {
        let errors = validate_description(Some(""));
        assert!(errors.iter().any(|e| e.contains("required")));
    }

    #[test]
    fn test_validate_description_too_long() {
        let long_desc = "a".repeat(MAX_DESCRIPTION_LENGTH + 1);
        let errors = validate_description(Some(&long_desc));
        assert!(errors.iter().any(|e| e.contains("exceeds")));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let (fm, body, has) = parse_frontmatter("just content");
        assert!(!has);
        assert_eq!(body, "just content");
        assert!(fm.name.is_none());
    }

    #[test]
    fn test_parse_frontmatter_with_fields() {
        let content = "---\nname: my-skill\ndescription: A test skill\ndisable-model-invocation: true\n---\n\nSkill content here";
        let (fm, body, has) = parse_frontmatter(content);
        assert!(has);
        assert_eq!(fm.name.unwrap(), "my-skill");
        assert_eq!(fm.description.unwrap(), "A test skill");
        assert_eq!(fm.disable_model_invocation, Some(true));
        assert!(body.contains("Skill content here"));
    }

    #[test]
    fn test_parse_frontmatter_partial() {
        let content = "---\nname: my-skill\n---\n\nContent";
        let (fm, body, has) = parse_frontmatter(content);
        assert!(has);
        assert_eq!(fm.name.unwrap(), "my-skill");
        assert!(fm.description.is_none());
        assert!(body.contains("Content"));
    }

    #[test]
    fn test_format_skills_for_prompt() {
        let skills = vec![Skill {
            name: "git-workflow".to_string(),
            description: "Git workflow helper".to_string(),
            file_path: "/home/user/.pi/agent/skills/git-workflow/SKILL.md".to_string(),
            base_dir: "/home/user/.pi/agent/skills/git-workflow".to_string(),
            source_info: create_skill_source_info(
                "/home/user/.pi/agent/skills/git-workflow/SKILL.md",
                "/home/user/.pi/agent/skills/git-workflow",
                "user",
            ),
            disable_model_invocation: false,
        }];
        let result = format_skills_for_prompt(&skills);
        assert!(result.contains("<available_skills>"));
        assert!(result.contains("git-workflow"));
        assert!(result.contains("</available_skills>"));
    }

    #[test]
    fn test_format_skills_empty() {
        let result = format_skills_for_prompt(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_skills_disabled() {
        let skills = vec![Skill {
            name: "internal".to_string(),
            description: "Internal skill".to_string(),
            file_path: "/path/SKILL.md".to_string(),
            base_dir: "/path".to_string(),
            source_info: create_skill_source_info("/path/SKILL.md", "/path", "path"),
            disable_model_invocation: true,
        }];
        let result = format_skills_for_prompt(&skills);
        assert!(!result.contains("internal"));
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(
            escape_xml("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
    }

    #[test]
    fn test_load_skills_empty() {
        let opts = LoadSkillsOptions {
            cwd: "/nonexistent".to_string(),
            include_defaults: false,
            ..Default::default()
        };
        let result = load_skills(&opts);
        assert!(result.skills.is_empty());
    }

    /// A skill file that is a symlink must be loaded, not treated as a
    /// directory. Regression: the discovery loop recursed into any symlink,
    /// so `<skills>/check-docs.md -> .../check-docs.md` produced
    /// "failed to read skills directory: Not a directory" and was dropped.
    #[cfg(unix)]
    #[test]
    fn test_symlinked_skill_file_is_discovered() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join(".pi-rs").join("skills");
        let target_dir = dir.path().join("target");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::create_dir_all(&target_dir).unwrap();
        let target = target_dir.join("check-docs.md");
        std::fs::write(
            &target,
            "---\nname: check-docs\ndescription: Check the docs\n---\n\n# Check\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, skills_dir.join("check-docs.md")).unwrap();

        let result = load_skills(&LoadSkillsOptions {
            cwd: dir.path().to_string_lossy().to_string(),
            agent_dir: Some(dir.path().join("agent").to_string_lossy().to_string()),
            include_defaults: true,
            // Keep the host's `~/.agents/skills` out of this fixture.
            home_dir: Some(dir.path().join("home").to_string_lossy().to_string()),
            ..Default::default()
        });
        let names: Vec<_> = result.skills.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"check-docs"),
            "symlinked skill not loaded; got {names:?}, diagnostics {:?}",
            result.diagnostics
        );
    }

    fn write_skill(dir: &Path, name: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\n\n# {name}\n"),
        )
        .unwrap();
    }

    /// `.agents/skills` must be discovered at both scopes (TS commit 39cbf47e4
    /// "discover skills in .agents paths by default"), with project resources
    /// ordered before user resources.
    #[test]
    fn test_load_skills_agents_directories() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("repo");
        let home = root.path().join("home");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        write_skill(
            &project.join(config::CONFIG_DIR_NAME).join("skills"),
            "from-pi-rs",
        );
        write_skill(
            &project.join(".agents").join("skills"),
            "from-project-agents",
        );
        write_skill(&home.join("agent").join("skills"), "from-agent-dir");
        write_skill(&home.join(".agents").join("skills"), "from-home-agents");

        let result = load_skills(&LoadSkillsOptions {
            cwd: project.to_string_lossy().to_string(),
            agent_dir: Some(home.join("agent").to_string_lossy().to_string()),
            include_defaults: true,
            home_dir: Some(home.to_string_lossy().to_string()),
            ..Default::default()
        });

        let names: Vec<&str> = result.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "from-pi-rs",
                "from-project-agents",
                "from-agent-dir",
                "from-home-agents"
            ],
            "unexpected order/diagnostics: {:?}",
            result.diagnostics
        );

        let scope = |name: &str| {
            result
                .skills
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.source_info.scope.clone())
                .unwrap()
        };
        assert_eq!(scope("from-project-agents"), SourceScope::Project);
        assert_eq!(scope("from-home-agents"), SourceScope::User);
        let source_base_dir = |name: &str| {
            result
                .skills
                .iter()
                .find(|s| s.name == name)
                .and_then(|s| s.source_info.base_dir.clone())
                .unwrap()
        };
        assert_eq!(
            PathBuf::from(source_base_dir("from-project-agents")),
            project.join(".agents")
        );
        assert_eq!(
            PathBuf::from(source_base_dir("from-home-agents")),
            home.join(".agents")
        );
    }

    /// TS `agents` discovery mode only accepts nested `SKILL.md` entries; a
    /// root-level Markdown file is not a skill and must not win a collision.
    #[test]
    fn test_agents_discovery_excludes_root_markdown_files() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("repo");
        let home = root.path().join("home");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let agents_skills = project.join(".agents").join("skills");
        std::fs::create_dir_all(&agents_skills).unwrap();
        std::fs::write(
            agents_skills.join("root-file.md"),
            "---\nname: root-file\ndescription: must not load\n---\n",
        )
        .unwrap();
        write_skill(&agents_skills, "nested-skill");
        let child_dir = agents_skills.join("third-party");
        std::fs::create_dir_all(&child_dir).unwrap();
        std::fs::write(
            child_dir.join("child-skill.md"),
            "---\nname: child-skill\ndescription: nested markdown\n---\n",
        )
        .unwrap();

        let result = load_skills(&LoadSkillsOptions {
            cwd: project.to_string_lossy().to_string(),
            agent_dir: Some(home.join("agent").to_string_lossy().to_string()),
            include_defaults: true,
            home_dir: Some(home.to_string_lossy().to_string()),
            ..Default::default()
        });

        assert!(result.skills.iter().any(|s| s.name == "nested-skill"));
        assert!(result.skills.iter().any(|s| s.name == "child-skill"));
        assert!(!result.skills.iter().any(|s| s.name == "root-file"));
    }

    /// Explicit paths use the session cwd as their base and support `~`, just
    /// like TS `resolvePath(rawPath, resolvedCwd, { trim: true })`.
    #[test]
    fn test_explicit_skill_paths_resolve_from_cwd_and_home() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let home = root.path().join("home");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        write_skill(&project.join("local-skill"), "local-skill");
        write_skill(&home.join("custom-skills"), "home-skill");

        let result = load_skills(&LoadSkillsOptions {
            cwd: project.to_string_lossy().to_string(),
            agent_dir: Some(home.join("agent").to_string_lossy().to_string()),
            skill_paths: vec![
                "local-skill".to_string(),
                "~/custom-skills".to_string(),
                "  ~/custom-skills  ".to_string(),
            ],
            include_defaults: false,
            home_dir: Some(home.to_string_lossy().to_string()),
        });

        assert!(result.skills.iter().any(|s| s.name == "local-skill"));
        assert_eq!(
            result
                .skills
                .iter()
                .filter(|s| s.name == "home-skill")
                .count(),
            1,
            "the same explicit path must not load twice"
        );
    }

    #[test]
    fn test_non_markdown_explicit_skill_path_warns() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let note = project.join("notes.txt");
        std::fs::write(&note, "not a skill").unwrap();

        let result = load_skills(&LoadSkillsOptions {
            cwd: project.to_string_lossy().to_string(),
            skill_paths: vec!["notes.txt".to_string()],
            include_defaults: false,
            ..Default::default()
        });

        assert!(result.diagnostics.iter().any(|d| matches!(
            d,
            ResourceDiagnostic::Warning { message, path }
                if message == "skill path is not a markdown file" && path == &note.to_string_lossy()
        )));
    }

    /// Node's `path.resolve` does not follow symlinks while walking ancestors.
    /// Canonicalizing first could escape the repository and miss its `.agents`.
    #[cfg(unix)]
    #[test]
    fn test_ancestor_walk_keeps_symlinked_cwd_lexical() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(outside.join("sub")).unwrap();
        std::os::unix::fs::symlink(&outside, repo.join("link")).unwrap();

        let cwd = repo.join("link").join("sub");
        let dirs = collect_ancestor_agents_skill_dirs(&cwd.to_string_lossy());
        assert!(
            dirs.iter().any(|dir| Path::new(dir) == repo.join(".agents").join("skills")),
            "git root must remain in the lexical walk: {dirs:?}"
        );
    }

    /// If `~/.agents/skills` is a symlink into the project, lexical filtering
    /// must not discard the project copy; canonical dedupe keeps project first.
    #[cfg(unix)]
    #[test]
    fn test_symlinked_home_agents_keeps_project_winner() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let home = root.path().join("home");
        let project_agents = repo.join(".agents");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(home.join("agent")).unwrap();
        write_skill(&project_agents.join("skills"), "shared");
        std::os::unix::fs::symlink(&project_agents, home.join(".agents")).unwrap();

        let result = load_skills(&LoadSkillsOptions {
            cwd: repo.join("sub").to_string_lossy().to_string(),
            agent_dir: Some(home.join("agent").to_string_lossy().to_string()),
            include_defaults: true,
            home_dir: Some(home.to_string_lossy().to_string()),
            ..Default::default()
        });

        let shared: Vec<_> = result
            .skills
            .iter()
            .filter(|s| s.name == "shared")
            .collect();
        assert_eq!(shared.len(), 1, "diagnostics: {:?}", result.diagnostics);
        assert_eq!(shared[0].source_info.scope, SourceScope::Project);
    }

    /// Project resources are added first by TS, so a project skill must win a
    /// name collision against a user skill (previously `HashMap` order decided).
    #[test]
    fn test_project_skill_wins_collision_against_user() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("repo");
        let home = root.path().join("home");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        write_skill(&project.join(config::CONFIG_DIR_NAME).join("skills"), "dup");
        write_skill(&home.join("agent").join("skills"), "dup");

        let result = load_skills(&LoadSkillsOptions {
            cwd: project.to_string_lossy().to_string(),
            agent_dir: Some(home.join("agent").to_string_lossy().to_string()),
            include_defaults: true,
            home_dir: Some(home.to_string_lossy().to_string()),
            ..Default::default()
        });

        let dups: Vec<&Skill> = result.skills.iter().filter(|s| s.name == "dup").collect();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].source_info.scope, SourceScope::Project);
        assert!(result.diagnostics.iter().any(|d| matches!(
            d,
            ResourceDiagnostic::Collision { collision, .. }
                if collision.winner_path.contains("repo")
        )));
    }

    /// The ancestor walk stops at (and includes) the git repository root.
    #[test]
    fn test_ancestor_agents_dirs_stop_at_git_root() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let nested = repo.join("pkg").join("crate");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();

        let dirs = collect_ancestor_agents_skill_dirs(&nested.to_string_lossy());
        assert_eq!(dirs.len(), 3, "dirs: {dirs:?}");

        let basename_of_grandparent = |dir: &str| {
            Path::new(dir)
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().to_string())
                .unwrap()
        };
        assert_eq!(basename_of_grandparent(&dirs[0]), "crate");
        assert_eq!(basename_of_grandparent(&dirs[1]), "pkg");
        assert_eq!(basename_of_grandparent(&dirs[2]), "repo");
    }

    #[test]
    fn test_lexical_resolution_normalizes_parent_components() {
        assert_eq!(
            resolve_absolute_path("/tmp/project/a/../b", None, false, None, false),
            "/tmp/project/b"
        );
        let cwd_relative_tilde = resolve_absolute_path("~/repo", None, false, None, false);
        assert_eq!(
            PathBuf::from(cwd_relative_tilde),
            std::env::current_dir().unwrap().join("~/repo")
        );
    }

    /// When `cwd` is not in a git repo the walk runs to the filesystem root, but
    /// the user-level `~/.agents/skills` must be loaded once, as a user resource.
    #[test]
    fn test_home_agents_dir_not_loaded_twice() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let project = home.join("repo");
        std::fs::create_dir_all(&project).unwrap();
        write_skill(&home.join(".agents").join("skills"), "shared");

        let result = load_skills(&LoadSkillsOptions {
            cwd: project.to_string_lossy().to_string(),
            agent_dir: Some(home.join("agent").to_string_lossy().to_string()),
            include_defaults: true,
            home_dir: Some(home.to_string_lossy().to_string()),
            ..Default::default()
        });

        let shared: Vec<&Skill> = result
            .skills
            .iter()
            .filter(|s| s.name == "shared")
            .collect();
        assert_eq!(shared.len(), 1, "diagnostics: {:?}", result.diagnostics);
        assert_eq!(shared[0].source_info.scope, SourceScope::User);
    }}
