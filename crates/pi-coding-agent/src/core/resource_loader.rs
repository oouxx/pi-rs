use std::path::Path;

use crate::config;
use crate::core::diagnostics::ResourceDiagnostic;
use crate::core::prompt_templates::{self, LoadPromptTemplatesOptions, PromptTemplate};
use crate::core::skills::{self, LoadSkillsOptions, Skill};

#[derive(Debug, Clone, Default)]
pub struct ResourceLoaderOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub include_defaults: bool,
    pub skill_paths: Vec<String>,
    pub prompt_paths: Vec<String>,
    pub extension_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedResources {
    pub skills: Vec<Skill>,
    pub prompt_templates: Vec<PromptTemplate>,
    pub context_files: Vec<ContextFile>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
}

pub fn load_all_resources(options: &ResourceLoaderOptions) -> LoadedResources {
    let mut all_diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    let skills_result = skills::load_skills(&LoadSkillsOptions {
        cwd: options.cwd.clone(),
        agent_dir: options.agent_dir.clone(),
        skill_paths: options.skill_paths.clone(),
        include_defaults: options.include_defaults,
    });
    all_diagnostics.extend(skills_result.diagnostics);

    let prompts_result = prompt_templates::load_prompt_templates(&LoadPromptTemplatesOptions {
        cwd: options.cwd.clone(),
        agent_dir: options.agent_dir.clone(),
        prompt_paths: options.prompt_paths.clone(),
        include_defaults: options.include_defaults,
    });
    all_diagnostics.extend(prompts_result.diagnostics);

    let context_files = load_context_files(&options.cwd);

    LoadedResources {
        skills: skills_result.skills,
        prompt_templates: prompts_result.templates,
        context_files,
        diagnostics: all_diagnostics,
    }
}

fn load_context_files(cwd: &str) -> Vec<ContextFile> {
    let mut files = Vec::new();
    let context_file_names = ["AGENTS.md", "CLAUDE.md", "PI.md", ".pi-instructions.md"];

    for name in &context_file_names {
        let path = Path::new(cwd).join(name);
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                files.push(ContextFile {
                    path: name.to_string(),
                    content,
                });
            }
        }
    }

    let pi_dir = Path::new(cwd).join(config::CONFIG_DIR_NAME);
    if pi_dir.exists() {
        let instructions_file = pi_dir.join("instructions.md");
        if instructions_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&instructions_file) {
                files.push(ContextFile {
                    path: format!("{}/instructions.md", config::CONFIG_DIR_NAME),
                    content,
                });
            }
        }
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
