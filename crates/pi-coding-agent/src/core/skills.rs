use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::core::diagnostics::ResourceDiagnostic;
use crate::core::source_info::{create_source_info, SourceInfo, SourceOrigin, SourceScope};

// ============================================================================
// Constants
// ============================================================================

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

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
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (
            SkillFrontmatter {
                name: None,
                description: None,
                disable_model_invocation: None,
            },
            content.to_string(),
            false,
        );
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let end_marker = after_first.find("\n---");
    if end_marker.is_none() {
        return (
            SkillFrontmatter {
                name: None,
                description: None,
                disable_model_invocation: None,
            },
            content.to_string(),
            true,
        );
    }

    let end_idx = end_marker.unwrap();
    let frontmatter_str = &after_first[..end_idx].trim();
    let body = after_first[end_idx + 4..].trim().to_string();

    // Parse YAML-like key: value pairs
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut disable_model_invocation: Option<bool> = None;

    for line in frontmatter_str.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().to_string();
            match key.trim() {
                "name" => name = Some(value),
                "description" => description = Some(value),
                "disable-model-invocation" => {
                    disable_model_invocation = Some(value == "true");
                }
                _ => {}
            }
        }
    }

    (
        SkillFrontmatter {
            name,
            description,
            disable_model_invocation,
        },
        body,
        true,
    )
}

// ============================================================================
// Source info creation
// ============================================================================

fn create_skill_source_info(file_path: &str, base_dir: &str, source: &str) -> SourceInfo {
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

/// Load skills from a directory, with recursive discovery.
///
/// Discovery rules:
/// - if a directory contains SKILL.md, treat it as a skill root and do not recurse further
/// - otherwise, load direct .md children in the root
/// - recurse into subdirectories to find SKILL.md
fn load_skills_from_dir(
    dir: &Path,
    source: &str,
    include_root_files: bool,
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
        if !full_path.is_file() {
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

        if full_path.is_dir() {
            let (sub_skills, sub_diags) = load_skills_from_dir(&full_path, source, false);
            skills.extend(sub_skills);
            diagnostics.extend(sub_diags);
        } else if include_root_files && full_path.is_file() {
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

/// Load skills from all configured locations.
pub fn load_skills(options: &LoadSkillsOptions) -> LoadSkillsResult {
    let resolved_agent_dir = options
        .agent_dir
        .as_deref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| config::get_agent_dir().to_string_lossy().to_string());

    let mut skill_map: HashMap<String, Skill> = HashMap::new();
    let mut real_path_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut all_diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    fn add_skills_to_map(
        skill_map: &mut HashMap<String, Skill>,
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

            if let Some(existing) = skill_map.get(&skill.name) {
                diagnostics.push(ResourceDiagnostic::Collision {
                    message: format!("name \"{}\" collision", skill.name),
                    path: skill.file_path.clone(),
                    collision: crate::core::diagnostics::ResourceCollision {
                        resource_type: "skill".to_string(),
                        name: skill.name.clone(),
                        winner_path: existing.file_path.clone(),
                        loser_path: skill.file_path.clone(),
                    },
                });
            } else {
                skill_map.insert(skill.name.clone(), skill);
                real_path_set.insert(real_path);
            }
        }
    }

    if options.include_defaults {
        let user_skills_dir = Path::new(&resolved_agent_dir).join("skills");
        add_skills_to_map(
            &mut skill_map,
            &mut real_path_set,
            &mut all_diagnostics,
            load_skills_from_dir(&user_skills_dir, "user", true),
        );

        let project_skills_dir = Path::new(&options.cwd)
            .join(config::CONFIG_DIR_NAME)
            .join("skills");
        add_skills_to_map(
            &mut skill_map,
            &mut real_path_set,
            &mut all_diagnostics,
            load_skills_from_dir(&project_skills_dir, "project", true),
        );
    }

    for raw_path in &options.skill_paths {
        let path = PathBuf::from(raw_path);
        if !path.exists() {
            all_diagnostics.push(ResourceDiagnostic::Warning {
                message: "skill path does not exist".to_string(),
                path: raw_path.clone(),
            });
            continue;
        }

        if path.is_dir() {
            add_skills_to_map(
                &mut skill_map,
                &mut real_path_set,
                &mut all_diagnostics,
                load_skills_from_dir(&path, "path", true),
            );
        } else if path.is_file() && raw_path.ends_with(".md") {
            let (skill, diags) = load_skill_from_file(raw_path, "path");
            all_diagnostics.extend(diags);
            if let Some(s) = skill {
                add_skills_to_map(
                    &mut skill_map,
                    &mut real_path_set,
                    &mut all_diagnostics,
                    (vec![s], vec![]),
                );
            }
        }
    }

    LoadSkillsResult {
        skills: skill_map.into_values().collect(),
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
    fn test_validate_description_none() {
        let errors = validate_description(None);
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
}
