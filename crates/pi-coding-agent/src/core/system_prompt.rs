use std::collections::HashSet;

use crate::config;

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
    pub instructions: String,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BuildSystemPromptOptions {
    pub cwd: String,
    pub custom_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub selected_tools: Option<Vec<String>>,
    pub tool_snippets: Option<std::collections::HashMap<String, String>>,
    pub prompt_guidelines: Option<Vec<String>>,
    pub context_files: Option<Vec<ContextFile>>,
    pub skills: Option<Vec<SkillInfo>>,
}

pub fn build_system_prompt(options: &BuildSystemPromptOptions) -> String {
    let cwd = &options.cwd;
    let custom_prompt = options.custom_prompt.as_deref();
    let append_system_prompt = options.append_system_prompt.as_deref();
    let selected_tools = options.selected_tools.as_ref();
    let tool_snippets = options.tool_snippets.as_ref();
    let prompt_guidelines = options.prompt_guidelines.as_ref();
    let context_files = options.context_files.as_ref().cloned().unwrap_or_default();
    let skills = options.skills.as_ref().cloned().unwrap_or_default();

    let prompt_cwd = cwd.replace('\\', "/");
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let append_section = append_system_prompt
        .map(|s| format!("\n\n{}", s))
        .unwrap_or_default();

    if let Some(custom) = custom_prompt {
        let mut prompt = custom.to_string();
        if !append_section.is_empty() {
            prompt.push_str(&append_section);
        }
        if !context_files.is_empty() {
            prompt.push_str("\n\n<project_context>\n\n");
            prompt.push_str("Project-specific instructions and guidelines:\n\n");
            for cf in &context_files {
                prompt.push_str(&format!(
                    "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                    cf.path, cf.content
                ));
            }
            prompt.push_str("</project_context>\n");
        }
        let has_read = selected_tools
            .as_ref()
            .map(|t| t.contains(&"read".to_string()))
            .unwrap_or(true);
        if has_read && !skills.is_empty() {
            prompt.push_str(&format_skills_for_prompt(&skills));
        }
        prompt.push_str(&format!("\nCurrent date: {}", date));
        prompt.push_str(&format!("\nCurrent working directory: {}", prompt_cwd));
        return prompt;
    }

    let default_tools: Vec<String> = ["read", "bash", "edit", "write"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let tools = selected_tools
        .as_ref()
        .map(|t| t.as_slice())
        .unwrap_or(&default_tools);

    let visible_tools: Vec<&String> = tools
        .iter()
        .filter(|name| tool_snippets.map(|sn| sn.contains_key(name.as_str())).unwrap_or(false))
        .collect();

    let tools_list = if !visible_tools.is_empty() {
        visible_tools
            .iter()
            .map(|name| {
                let snippet = tool_snippets
                    .as_ref()
                    .and_then(|sn| sn.get(name.as_str()))
                    .map(|s| s.as_str())
                    .unwrap_or("");
                format!("- {}: {}", name, snippet)
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        "(none)".to_string()
    };

    let mut guidelines_list: Vec<String> = Vec::new();
    let mut guidelines_set: HashSet<String> = HashSet::new();

    let mut add_guideline = |guideline: &str| {
        if guidelines_set.insert(guideline.to_string()) {
            guidelines_list.push(guideline.to_string());
        }
    };

    let has_bash = tools.iter().any(|t| t == "bash");
    let has_grep = tools.iter().any(|t| t == "grep");
    let has_find = tools.iter().any(|t| t == "find");
    let has_ls = tools.iter().any(|t| t == "ls");
    let has_read = tools.iter().any(|t| t == "read");

    if has_bash && !has_grep && !has_find && !has_ls {
        add_guideline("Use bash for file operations like ls, rg, find");
    }

    if let Some(guidelines) = prompt_guidelines {
        for guideline in guidelines {
            let normalized = guideline.trim();
            if !normalized.is_empty() {
                add_guideline(normalized);
            }
        }
    }

    add_guideline("Be concise in your responses");
    add_guideline("Show file paths clearly when working with files");

    let guidelines = guidelines_list
        .iter()
        .map(|g| format!("- {}", g))
        .collect::<Vec<_>>()
        .join("\n");

    let readme_path = get_readme_path();
    let docs_path = get_docs_path();
    let examples_path = get_examples_path();

    let mut prompt = format!(
        r#"You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

Available tools:
{tools_list}

In addition to the tools above, you may have access to other custom tools depending on the project.

Guidelines:
{guidelines}

Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):
- Main documentation: {readme_path}
- Additional docs: {docs_path}
- Examples: {examples_path} (extensions, custom tools, SDK)
- When reading pi docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory
- When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), pi packages (docs/packages.md)
- When working on pi topics, read the docs and examples and follow .md cross-references before implementing
- Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)"#,
        tools_list = tools_list,
        guidelines = guidelines,
        readme_path = readme_path,
        docs_path = docs_path,
        examples_path = examples_path,
    );

    if !append_section.is_empty() {
        prompt.push_str(&append_section);
    }

    if !context_files.is_empty() {
        prompt.push_str("\n\n<project_context>\n\n");
        prompt.push_str("Project-specific instructions and guidelines:\n\n");
        for cf in &context_files {
            prompt.push_str(&format!(
                "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                cf.path, cf.content
            ));
        }
        prompt.push_str("</project_context>\n");
    }

    if has_read && !skills.is_empty() {
        prompt.push_str(&format_skills_for_prompt(&skills));
    }

    prompt.push_str(&format!("\nCurrent date: {}", date));
    prompt.push_str(&format!("\nCurrent working directory: {}", prompt_cwd));

    prompt
}

fn format_skills_for_prompt(skills: &[SkillInfo]) -> String {
    let mut result = String::from("\n\n<skills>\n\n");
    result.push_str("You have access to the following skills. Use them when relevant:\n\n");
    for skill in skills {
        result.push_str(&format!("### Skill: {}\n", skill.name));
        if !skill.description.is_empty() {
            result.push_str(&format!("{}\n", skill.description));
        }
        if !skill.instructions.is_empty() {
            result.push_str(&format!("\nInstructions:\n{}\n", skill.instructions));
        }
        if !skill.tools.is_empty() {
            result.push_str(&format!("\nTools: {}\n", skill.tools.join(", ")));
        }
        result.push('\n');
    }
    result.push_str("</skills>\n");
    result
}

fn get_readme_path() -> String {
    let agent_dir = config::get_agent_dir();
    agent_dir
        .parent()
        .map(|p| p.join("README.md"))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "README.md".to_string())
}

fn get_docs_path() -> String {
    let agent_dir = config::get_agent_dir();
    agent_dir
        .parent()
        .map(|p| p.join("docs"))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "docs".to_string())
}

fn get_examples_path() -> String {
    let agent_dir = config::get_agent_dir();
    agent_dir
        .parent()
        .map(|p| p.join("examples"))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "examples".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt_default() {
        let opts = BuildSystemPromptOptions {
            cwd: "/home/user/project".to_string(),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("expert coding assistant"));
        assert!(prompt.contains("Available tools:"));
        assert!(prompt.contains("Current working directory: /home/user/project"));
        assert!(prompt.contains("Current date:"));
    }

    #[test]
    fn test_build_system_prompt_custom() {
        let opts = BuildSystemPromptOptions {
            cwd: "/home/user/project".to_string(),
            custom_prompt: Some("You are a Rust expert.".to_string()),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Rust expert"));
        assert!(prompt.contains("Current working directory: /home/user/project"));
    }

    #[test]
    fn test_build_system_prompt_with_append() {
        let opts = BuildSystemPromptOptions {
            cwd: "/home/user/project".to_string(),
            append_system_prompt: Some("Always use Rust idioms.".to_string()),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Always use Rust idioms."));
    }

    #[test]
    fn test_build_system_prompt_with_context_files() {
        let opts = BuildSystemPromptOptions {
            cwd: "/home/user/project".to_string(),
            context_files: Some(vec![ContextFile {
                path: "AGENTS.md".to_string(),
                content: "Use tabs for indentation".to_string(),
            }]),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("<project_context>"));
        assert!(prompt.contains("AGENTS.md"));
        assert!(prompt.contains("Use tabs for indentation"));
        assert!(prompt.contains("</project_context>"));
    }

    #[test]
    fn test_build_system_prompt_with_skills() {
        let opts = BuildSystemPromptOptions {
            cwd: "/home/user/project".to_string(),
            skills: Some(vec![SkillInfo {
                name: "git-workflow".to_string(),
                description: "Git workflow helper".to_string(),
                instructions: "Always create feature branches".to_string(),
                tools: vec!["bash".to_string()],
            }]),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("<skills>"));
        assert!(prompt.contains("git-workflow"));
        assert!(prompt.contains("</skills>"));
    }

    #[test]
    fn test_build_system_prompt_with_guidelines() {
        let opts = BuildSystemPromptOptions {
            cwd: "/home/user/project".to_string(),
            prompt_guidelines: Some(vec![
                "Always write tests".to_string(),
                "Use descriptive variable names".to_string(),
            ]),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Always write tests"));
        assert!(prompt.contains("Use descriptive variable names"));
    }

    #[test]
    fn test_build_system_prompt_bash_guideline() {
        let opts = BuildSystemPromptOptions {
            cwd: "/home/user/project".to_string(),
            selected_tools: Some(vec!["bash".to_string(), "read".to_string()]),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Use bash for file operations like ls, rg, find"));
    }
}