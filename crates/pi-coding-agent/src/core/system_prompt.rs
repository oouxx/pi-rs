//! System prompt construction (port of
//! `packages/coding-agent/src/core/system-prompt.ts`, TS 9e05370b2).
//!
//! The prompt is an ordered set of named sections. `preamble` is untagged text;
//! every other section is wrapped in a tag of the same name so the model can
//! match later updates to it. These become `SystemMessage.sections` in the
//! transcript. `build_system_prompt` renders the exact text the transcript's
//! system message replays.

use std::collections::HashSet;

use indexmap::IndexMap;

pub const DEFAULT_THINKING_LEVEL: &str = "medium";

#[derive(Debug, Clone, Default)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub base_dir: String,
}

#[derive(Debug, Clone, Default)]
pub struct BuildSystemPromptOptions {
    pub cwd: String,
    /// Custom system prompt (replaces the default preamble).
    pub custom_prompt: Option<String>,
    /// Exact full prompt replacement set by a `before_agent_start` handler.
    pub force_system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub selected_tools: Option<Vec<String>>,
    pub tool_snippets: Option<std::collections::HashMap<String, String>>,
    /// Guideline bullets contributed by each tool, keyed by tool name.
    pub tool_guidelines: Option<std::collections::HashMap<String, Vec<String>>>,
    pub prompt_guidelines: Option<Vec<String>>,
    /// Additional XML-wrapped prompt sections keyed by tag name.
    pub sections: Option<IndexMap<String, String>>,
    pub context_files: Option<Vec<ContextFile>>,
    pub skills: Option<Vec<SkillInfo>>,
}

/// Ordered prompt sections (`preamble` untagged, others tag-wrapped). Insertion
/// order is the render order.
pub type SystemPromptSections = IndexMap<String, Option<String>>;

fn valid_section_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// TS `buildRules`.
fn build_rules(
    selected_tools: &[String],
    tool_guidelines: &std::collections::HashMap<String, Vec<String>>,
    prompt_guidelines: &[String],
) -> String {
    let mut rules: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add_rule = |rule: &str| {
        let normalized = rule.trim();
        if normalized.is_empty() || seen.contains(normalized) {
            return;
        }
        seen.insert(normalized.to_string());
        rules.push(normalized.to_string());
    };

    let has_bash = selected_tools.iter().any(|t| t == "bash");
    let has_powershell = selected_tools.iter().any(|t| t == "powershell");
    let has_grep = selected_tools.iter().any(|t| t == "grep");
    let has_find = selected_tools.iter().any(|t| t == "find");
    let has_ls = selected_tools.iter().any(|t| t == "ls");

    if (has_bash || has_powershell) && !has_grep && !has_find && !has_ls {
        if has_bash && has_powershell {
            add_rule("Use bash or PowerShell for file operations like listing, searching, and finding files");
        } else if has_powershell {
            add_rule(
                "Use PowerShell for file operations like listing, searching, and finding files",
            );
        } else {
            add_rule("Use bash for file operations like ls, rg, find");
        }
    }
    for name in selected_tools {
        if let Some(guidelines) = tool_guidelines.get(name) {
            for rule in guidelines {
                add_rule(rule);
            }
        }
    }
    for rule in prompt_guidelines {
        add_rule(rule);
    }
    add_rule("Be concise in your responses");
    add_rule("Show file paths clearly when working with files");
    rules
        .iter()
        .map(|r| format!("- {r}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_project_context(context_files: &[ContextFile]) -> String {
    let mut parts = vec!["Project-specific instructions and guidelines:".to_string()];
    for cf in context_files {
        parts.push(format!(
            "<project_instructions path=\"{}\">\n{}\n</project_instructions>",
            cf.path, cf.content
        ));
    }
    parts.join("\n\n")
}

/// TS `buildSystemPromptSections`: ordered, independently replaceable sections.
///
/// # Errors
/// Returns `Err` when a custom section name is invalid (`preamble` or not
/// `^[a-z][a-z0-9_-]*$`).
pub fn build_system_prompt_sections(
    options: &BuildSystemPromptOptions,
) -> Result<SystemPromptSections, String> {
    if let Some(custom) = &options.sections {
        for name in custom.keys() {
            if !valid_section_name(name) || name == "preamble" {
                return Err(format!("Invalid system prompt section name: {name}"));
            }
        }
    }

    let custom_prompt = options.custom_prompt.as_deref();
    let selected_tools: Vec<String> = options.selected_tools.clone().unwrap_or_else(|| {
        ["read", "bash", "edit", "write"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    });
    let tool_snippets = options.tool_snippets.clone().unwrap_or_default();
    let tool_guidelines = options.tool_guidelines.clone().unwrap_or_default();
    let prompt_guidelines = options.prompt_guidelines.clone().unwrap_or_default();
    let context_files = options.context_files.clone().unwrap_or_default();
    let skills = options.skills.clone().unwrap_or_default();

    let mut sections: SystemPromptSections = IndexMap::new();
    let preamble = match custom_prompt {
        Some(custom) => custom.to_string(),
        None => {
            let visible: Vec<&String> = selected_tools
                .iter()
                .filter(|name| tool_snippets.contains_key(name.as_str()))
                .collect();
            let tools = if visible.is_empty() {
                "(none)".to_string()
            } else {
                visible
                    .iter()
                    .map(|name| {
                        let snippet = tool_snippets
                            .get(name.as_str())
                            .cloned()
                            .unwrap_or_default();
                        format!("- {name}: {snippet}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            sections.insert(
                "tools".to_string(),
                Some(format!(
                    "{tools}\n\nIn addition to the tools above, you may have access to other custom tools depending on the project."
                )),
            );
            sections.insert(
                "rules".to_string(),
                Some(build_rules(
                    &selected_tools,
                    &tool_guidelines,
                    &prompt_guidelines,
                )),
            );
            "You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.".to_string()
        }
    };

    if let Some(addendum) = options.append_system_prompt.as_deref() {
        if !addendum.is_empty() {
            sections.insert("addendum".to_string(), Some(addendum.to_string()));
        }
    }
    if !context_files.is_empty() {
        sections.insert(
            "project_context".to_string(),
            Some(render_project_context(&context_files)),
        );
    }
    let skill_file_read_tool = ["read", "bash"]
        .iter()
        .find(|tool| selected_tools.iter().any(|t| t == *tool));
    if let Some(_tool) = skill_file_read_tool {
        if !skills.is_empty() {
            let skills_prompt = format_skills_for_prompt(&skills).trim().to_string();
            if !skills_prompt.is_empty() {
                sections.insert("skills".to_string(), Some(skills_prompt));
            }
        }
    }
    sections.insert("cwd".to_string(), Some(options.cwd.replace('\\', "/")));
    if let Some(custom) = &options.sections {
        for (name, content) in custom {
            if !content.is_empty() {
                sections.insert(name.clone(), Some(content.clone()));
            }
        }
    }

    // `preamble` stays untagged; every other section is tag-wrapped.
    let mut out: SystemPromptSections = IndexMap::new();
    out.insert("preamble".to_string(), Some(preamble));
    for (name, content) in sections {
        if let Some(content) = content {
            if name == "preamble" {
                out.insert(name, Some(content));
            } else {
                out.insert(
                    name.clone(),
                    Some(format!("<{name}>\n{content}\n</{name}>")),
                );
            }
        }
    }
    Ok(out)
}

/// TS `buildSystemPromptState`: `(content, sections)`.
///
/// A forced prompt is opaque (`content` only); otherwise `content` is empty and
/// the structured sections carry the prompt.
#[must_use]
pub fn build_system_prompt_state(
    options: &BuildSystemPromptOptions,
) -> (String, Option<SystemPromptSections>) {
    if let Some(forced) = &options.force_system_prompt {
        return (forced.clone(), None);
    }
    match build_system_prompt_sections(options) {
        Ok(sections) => (String::new(), Some(sections)),
        Err(_) => (String::new(), None),
    }
}

/// Render sections exactly as `getSystemMessageText` does: content, then each
/// non-null section, joined with blank lines, empty parts dropped.
fn render_message(content: &str, sections: &SystemPromptSections) -> String {
    let mut parts: Vec<String> = vec![content.to_string()];
    for value in sections.values().flatten() {
        parts.push(value.clone());
    }
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Build the system prompt text, rendered exactly as the transcript's system
/// message replays it.
#[must_use]
pub fn build_system_prompt(options: &BuildSystemPromptOptions) -> String {
    let (content, sections) = build_system_prompt_state(options);
    match sections {
        Some(sections) => render_message(&content, &sections),
        None => content,
    }
}

/// Build the leading transcript system message carrying the prompt sections.
#[must_use]
pub fn build_system_prompt_message(
    options: &BuildSystemPromptOptions,
) -> crate::pi_agent_core::types::AgentMessage {
    let (content, sections) = build_system_prompt_state(options);
    crate::pi_agent_core::types::AgentMessage::System {
        content: crate::pi_agent_core::pi_ai_types::SystemContent::Text(content),
        sections,
        tools_added: None,
        tools_removed: None,
        timestamp: 0,
    }
}

/// TS `diffSystemPromptSections`: a `SystemMessage.sections` patch, or `None`
/// when nothing changed. Missing entries become `null` (removed).
#[must_use]
pub fn diff_system_prompt_sections(
    previous: &IndexMap<String, Option<String>>,
    current: &SystemPromptSections,
) -> Option<IndexMap<String, Option<String>>> {
    let mut patch: IndexMap<String, Option<String>> = IndexMap::new();
    for (name, text) in current {
        if previous.get(name).and_then(|v| v.as_deref()) != text.as_deref() {
            patch.insert(name.clone(), text.clone());
        }
    }
    for name in previous.keys() {
        if !current.contains_key(name) {
            patch.insert(name.clone(), None);
        }
    }
    if patch.is_empty() {
        None
    } else {
        Some(patch)
    }
}

pub fn format_skills_for_prompt(skills: &[SkillInfo]) -> String {
    let mut result = String::from("\n\n<skills>\n\n");
    result.push_str("You have access to the following skills. Use them when relevant:\n\n");
    for skill in skills {
        result.push_str(&format!("### Skill: {}\n", skill.name));
        if !skill.description.is_empty() {
            result.push_str(&format!("{}\n", skill.description));
        }
        result.push('\n');
    }
    result.push_str("</skills>\n");
    result
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn base() -> BuildSystemPromptOptions {
        BuildSystemPromptOptions {
            cwd: "/tmp".to_string(),
            selected_tools: Some(vec![]),
            context_files: Some(vec![]),
            skills: Some(vec![]),
            ..Default::default()
        }
    }

    #[test]
    fn default_prompt_uses_tagged_sections() {
        let prompt = build_system_prompt(&base());
        assert!(prompt.starts_with("You are an expert coding assistant operating inside pi"));
        assert!(prompt.contains("<tools>\n(none)\n"));
        assert!(prompt.contains("<cwd>\n/tmp\n</cwd>"));
        assert!(prompt.contains("Show file paths clearly"));
        // The TS default no longer emits a date.
        assert!(!prompt.contains("Current date:"));
    }

    #[test]
    fn custom_prompt_preamble_then_cwd() {
        let opts = BuildSystemPromptOptions {
            custom_prompt: Some("You are Exact.".to_string()),
            ..base()
        };
        let prompt = build_system_prompt(&opts);
        assert!(
            prompt.starts_with("You are Exact.\n\n<cwd>"),
            "got: {prompt}"
        );
        // Custom prompt replaces tools/rules.
        assert!(!prompt.contains("<tools>"));
    }

    #[test]
    fn forced_prompt_is_exact() {
        let opts = BuildSystemPromptOptions {
            force_system_prompt: Some("exact".to_string()),
            ..base()
        };
        assert_eq!(build_system_prompt(&opts), "exact");
        match build_system_prompt_message(&opts) {
            crate::pi_agent_core::types::AgentMessage::System { sections, .. } => {
                assert!(sections.is_none());
            }
            other => panic!("expected system, got {other:?}"),
        }
    }

    #[test]
    fn appended_instructions_and_context_map_to_sections() {
        let opts = BuildSystemPromptOptions {
            custom_prompt: Some("You are Exact.".to_string()),
            append_system_prompt: Some("Additional instructions.".to_string()),
            context_files: Some(vec![ContextFile {
                path: "/tmp/AGENTS.md".to_string(),
                content: "Project instructions.".to_string(),
            }]),
            ..base()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("<addendum>\nAdditional instructions.\n</addendum>"));
        assert!(prompt.contains(
            "<project_context>\nProject-specific instructions and guidelines:\n\n<project_instructions path=\"/tmp/AGENTS.md\">"
        ));
    }

    #[test]
    fn sections_are_ordered_and_tag_wrapped() {
        let mut tool_snippets = std::collections::HashMap::new();
        tool_snippets.insert("read".to_string(), "Read file contents".to_string());
        let opts = BuildSystemPromptOptions {
            tool_snippets: Some(tool_snippets),
            ..base()
        };
        let sections = build_system_prompt_sections(&opts).unwrap();
        let names: Vec<&str> = sections.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            vec!["preamble", "tools", "rules", "cwd"],
            "insertion order preserved"
        );
        assert!(sections["tools"].as_deref().unwrap().starts_with("<tools>"));
    }

    #[test]
    fn invalid_section_name_errors() {
        let mut custom = IndexMap::new();
        custom.insert("Bad Name".to_string(), "x".to_string());
        let opts = BuildSystemPromptOptions {
            sections: Some(custom),
            ..base()
        };
        assert!(build_system_prompt_sections(&opts).is_err());
    }

    #[test]
    fn diff_reports_changes_and_removals() {
        let mut previous: IndexMap<String, Option<String>> = IndexMap::new();
        previous.insert("cwd".to_string(), Some("<cwd>\n/a\n</cwd>".to_string()));
        previous.insert(
            "skills".to_string(),
            Some("<skills>old</skills>".to_string()),
        );
        let mut current: SystemPromptSections = IndexMap::new();
        current.insert("cwd".to_string(), Some("<cwd>\n/b\n</cwd>".to_string()));
        let patch = diff_system_prompt_sections(&previous, &current).unwrap();
        assert_eq!(patch["cwd"].as_deref(), Some("<cwd>\n/b\n</cwd>"));
        assert_eq!(patch["skills"], None, "missing section removed");
    }
}
