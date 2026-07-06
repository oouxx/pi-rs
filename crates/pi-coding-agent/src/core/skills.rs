use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::core::diagnostics::ResourceDiagnostic;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub file_path: String,
    pub source: SkillSource,
    pub disable_model_invocation: bool,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    User,
    Project,
    Path,
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

pub fn load_skills(options: &LoadSkillsOptions) -> LoadSkillsResult {
    let resolved_agent_dir = options
        .agent_dir
        .as_deref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| config::get_agent_dir().to_string_lossy().to_string());

    let mut skill_map: HashMap<String, Skill> = HashMap::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    if options.include_defaults {
        let user_skills_dir = Path::new(&resolved_agent_dir).join("skills");
        if user_skills_dir.exists() {
            load_skills_from_dir(
                &user_skills_dir,
                SkillSource::User,
                &mut skill_map,
                &mut diagnostics,
            );
        }

        let project_skills_dir = Path::new(&options.cwd)
            .join(config::CONFIG_DIR_NAME)
            .join("skills");
        if project_skills_dir.exists() {
            load_skills_from_dir(
                &project_skills_dir,
                SkillSource::Project,
                &mut skill_map,
                &mut diagnostics,
            );
        }
    }

    for raw_path in &options.skill_paths {
        let path = PathBuf::from(raw_path);
        if !path.exists() {
            diagnostics.push(ResourceDiagnostic::Warning {
                message: "skill path does not exist".to_string(),
                path: raw_path.clone(),
            });
            continue;
        }

        if path.is_dir() {
            load_skills_from_dir(&path, SkillSource::Path, &mut skill_map, &mut diagnostics);
        } else if path.is_file() && raw_path.ends_with(".md") {
            if let Some(skill) = load_skill_from_file(&path, SkillSource::Path) {
                skill_map.insert(skill.name.clone(), skill);
            }
        }
    }

    LoadSkillsResult {
        skills: skill_map.into_values().collect(),
        diagnostics,
    }
}

fn load_skills_from_dir(
    dir: &Path,
    source: SkillSource,
    skill_map: &mut HashMap<String, Skill>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            diagnostics.push(ResourceDiagnostic::Warning {
                message: format!("failed to read skills directory: {}", e),
                path: dir.to_string_lossy().to_string(),
            });
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skill_file = path.join("SKILL.md");
            if skill_file.exists() {
                if let Some(skill) = load_skill_from_file(&skill_file, source.clone()) {
                    skill_map.insert(skill.name.clone(), skill);
                }
            }
        } else if path.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
            if let Some(skill) = load_skill_from_file(&path, source.clone()) {
                skill_map.insert(skill.name.clone(), skill);
            }
        }
    }
}

fn load_skill_from_file(path: &Path, source: SkillSource) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;
    let file_name = path.file_stem()?.to_str()?.to_string();
    let description = extract_description(&content).unwrap_or_default();
    let instructions = content.clone();

    Some(Skill {
        name: file_name,
        description,
        instructions,
        file_path: path.to_string_lossy().to_string(),
        source,
        disable_model_invocation: content.contains("disableModelInvocation: true"),
        tools: extract_tools(&content),
    })
}

fn extract_description(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        return Some(trimmed.to_string());
    }
    None
}

fn extract_tools(content: &str) -> Vec<String> {
    let mut tools = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("tools:") {
            let tool_list = rest.trim();
            for tool in tool_list.split(',') {
                let t = tool.trim().to_string();
                if !t.is_empty() {
                    tools.push(t);
                }
            }
        }
    }
    tools
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_skills_for_prompt() {
        let skills = vec![Skill {
            name: "git-workflow".to_string(),
            description: "Git workflow helper".to_string(),
            instructions: "Use git commands to help with workflows".to_string(),
            file_path: "/home/user/.pi/agent/skills/git-workflow/SKILL.md".to_string(),
            source: SkillSource::User,
            disable_model_invocation: false,
            tools: vec!["bash".to_string()],
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
            instructions: "Internal processing instructions".to_string(),
            file_path: "/path/SKILL.md".to_string(),
            source: SkillSource::Path,
            disable_model_invocation: true,
            tools: vec![],
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
}
