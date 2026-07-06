use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::core::diagnostics::ResourceDiagnostic;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub source: PromptSource,
    pub append: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PromptSource {
    User,
    Project,
    Path,
}

#[derive(Debug, Clone, Default)]
pub struct LoadPromptTemplatesOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub prompt_paths: Vec<String>,
    pub include_defaults: bool,
}

#[derive(Debug, Clone)]
pub struct LoadPromptTemplatesResult {
    pub templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

pub fn load_prompt_templates(options: &LoadPromptTemplatesOptions) -> LoadPromptTemplatesResult {
    let resolved_agent_dir = options
        .agent_dir
        .as_deref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| config::get_agent_dir().to_string_lossy().to_string());

    let mut template_map: HashMap<String, PromptTemplate> = HashMap::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    if options.include_defaults {
        let user_prompts_dir = Path::new(&resolved_agent_dir).join("prompts");
        if user_prompts_dir.exists() {
            load_prompts_from_dir(
                &user_prompts_dir,
                PromptSource::User,
                &mut template_map,
                &mut diagnostics,
            );
        }

        let project_prompts_dir = Path::new(&options.cwd)
            .join(config::CONFIG_DIR_NAME)
            .join("prompts");
        if project_prompts_dir.exists() {
            load_prompts_from_dir(
                &project_prompts_dir,
                PromptSource::Project,
                &mut template_map,
                &mut diagnostics,
            );
        }
    }

    for raw_path in &options.prompt_paths {
        let path = std::path::PathBuf::from(raw_path);
        if !path.exists() {
            diagnostics.push(ResourceDiagnostic::Warning {
                message: "prompt path does not exist".to_string(),
                path: raw_path.clone(),
            });
            continue;
        }

        if path.is_dir() {
            load_prompts_from_dir(
                &path,
                PromptSource::Path,
                &mut template_map,
                &mut diagnostics,
            );
        } else if path.is_file() && raw_path.ends_with(".md") {
            if let Some(template) = load_prompt_from_file(&path, PromptSource::Path) {
                template_map.insert(template.name.clone(), template);
            }
        }
    }

    LoadPromptTemplatesResult {
        templates: template_map.into_values().collect(),
        diagnostics,
    }
}

fn load_prompts_from_dir(
    dir: &Path,
    source: PromptSource,
    template_map: &mut HashMap<String, PromptTemplate>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            diagnostics.push(ResourceDiagnostic::Warning {
                message: format!("failed to read prompts directory: {}", e),
                path: dir.to_string_lossy().to_string(),
            });
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
            if let Some(template) = load_prompt_from_file(&path, source.clone()) {
                template_map.insert(template.name.clone(), template);
            }
        }
    }
}

fn load_prompt_from_file(path: &Path, source: PromptSource) -> Option<PromptTemplate> {
    let content = std::fs::read_to_string(path).ok()?;
    let file_name = path.file_stem()?.to_str()?.to_string();
    let description = extract_description(&content).unwrap_or_default();
    let append = content.contains("append: true");

    Some(PromptTemplate {
        name: file_name,
        description,
        file_path: path.to_string_lossy().to_string(),
        source,
        append,
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

pub fn read_prompt_content(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("Failed to read prompt file: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_prompt_templates_empty() {
        let opts = LoadPromptTemplatesOptions {
            cwd: "/nonexistent".to_string(),
            include_defaults: false,
            ..Default::default()
        };
        let result = load_prompt_templates(&opts);
        assert!(result.templates.is_empty());
    }
}
