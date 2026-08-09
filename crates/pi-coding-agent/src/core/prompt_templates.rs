use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::core::diagnostics::ResourceDiagnostic;
use crate::core::source_info::{SourceInfo, create_synthetic_source_info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub source: PromptSource,
    pub append: bool,
    pub source_info: SourceInfo,
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

    let source_info = create_synthetic_source_info(
        path.to_string_lossy().to_string(),
        "local".to_string(),
        None,
        None,
        None,
    );

    Some(PromptTemplate {
        name: file_name,
        description,
        file_path: path.to_string_lossy().to_string(),
        source,
        append,
        source_info,
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

/// Parse command arguments respecting quoted strings (bash-style), matching
/// TS `parseCommandArgs` in `prompt-templates.ts`.
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    for ch in args_string.chars() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' | '\'' => in_quote = Some(ch),
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Substitute argument placeholders in template content, matching TS
/// `substituteArgs` in `prompt-templates.ts`:
/// - `${N:-default}` / `${@:-default}` / `${ARGUMENTS:-default}` — arg with fallback
/// - `${@:start}` / `${@:start:length}` — argument slice (1-indexed)
/// - `$ARGUMENTS` / `$@` — all args joined
/// - `$N` — the N-th argument (1-indexed), empty when missing
pub fn substitute_args(content: &str, args: &[String]) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    #[allow(clippy::unwrap_used)] // static literal pattern, compilation is infallible
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"\$\{(\d+|ARGUMENTS|@):-([^}]*)\}|\$\{@:(\d+)(?::(\d+))?\}|\$(ARGUMENTS|@|\d+)",
        )
        .unwrap()
    });
    let all_args = args.join(" ");
    re.replace_all(content, |caps: &regex::Captures| {
        if let Some(target) = caps.get(1) {
            // ${N:-default} / ${@:-default} / ${ARGUMENTS:-default}
            let default_value = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let value = if target.as_str() == "@" || target.as_str() == "ARGUMENTS" {
                all_args.clone()
            } else {
                target
                    .as_str()
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| args.get(n - 1).cloned())
                    .unwrap_or_default()
            };
            if value.is_empty() {
                default_value.to_string()
            } else {
                value
            }
        } else if let Some(slice_start) = caps.get(3) {
            // ${@:start[:length]}
            let mut start = slice_start.as_str().parse::<usize>().unwrap_or(1);
            if start == 0 {
                start = 1; // bash convention: args start at 1
            }
            let start_idx = start - 1;
            let sliced: Vec<String> = match caps.get(4) {
                Some(len) => args
                    .iter()
                    .skip(start_idx)
                    .take(len.as_str().parse::<usize>().unwrap_or(0))
                    .cloned()
                    .collect(),
                None => args.iter().skip(start_idx).cloned().collect(),
            };
            sliced.join(" ")
        } else if let Some(simple) = caps.get(5) {
            // $ARGUMENTS / $@ / $N
            if simple.as_str() == "ARGUMENTS" || simple.as_str() == "@" {
                all_args.clone()
            } else {
                simple
                    .as_str()
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| args.get(n - 1).cloned())
                    .unwrap_or_default()
            }
        } else {
            String::new()
        }
    })
    .into_owned()
}

/// Expand a leading `/templateName args` using the loaded prompt templates,
/// matching TS `expandPromptTemplate`. Returns the original text when the
/// text is not a known template command. Template content is read from disk
/// (frontmatter stripped) and `$1`/`$2`/`$@`/`${@:n}` placeholders substituted.
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
    if !text.starts_with('/') {
        return text.to_string();
    }
    // Match TS regex ^\/([^\s]+)(?:\s+([\s\S]*))?$
    let rest = &text[1..];
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let template_name = &rest[..name_end];
    let args_string = rest[name_end..].trim_start();

    let Some(template) = templates.iter().find(|t| t.name == template_name) else {
        return text.to_string();
    };
    let Ok(content) = read_prompt_content(&template.file_path) else {
        return text.to_string();
    };
    let body = crate::utils::frontmatter::strip_frontmatter(&content)
        .trim()
        .to_string();
    let args = parse_command_args(args_string);
    substitute_args(&body, &args)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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

    #[test]
    fn parse_command_args_handles_quotes() {
        assert_eq!(parse_command_args("a b c"), vec!["a", "b", "c"]);
        assert_eq!(parse_command_args("a \"b c\" d"), vec!["a", "b c", "d"]);
        assert_eq!(parse_command_args("'x y'"), vec!["x y"]);
        assert_eq!(parse_command_args("  "), Vec::<String>::new());
    }

    /// `substituteArgs` must match TS behavior: `$N`, `$@`, `$ARGUMENTS`,
    /// `${N:-default}` fallbacks and `${@:start[:length]}` slices.
    #[test]
    fn substitute_args_matches_ts() {
        let args = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        // Simple positional
        assert_eq!(substitute_args("$1 and $2", &args), "one and two");
        // All args
        assert_eq!(substitute_args("all: $@", &args), "all: one two three");
        assert_eq!(substitute_args("all: $ARGUMENTS", &args), "all: one two three");
        // Missing arg -> empty
        assert_eq!(substitute_args("$9", &args), "");
        // Default fallback: ${2:-fallback} when arg present uses the arg
        assert_eq!(substitute_args("${2:-fallback}", &args), "two");
        // Default fallback fires when arg is missing
        assert_eq!(substitute_args("${9:-fallback}", &args), "fallback");
        assert_eq!(substitute_args("${@:-nope}", &[]), "nope");
        // Slice: ${@:2} = args from index 2, ${@:2:1} = args[1..2]
        assert_eq!(substitute_args("${@:2}", &args), "two three");
        assert_eq!(substitute_args("${@:2:1}", &args), "two");
        // Slice with 0 start treated as 1 (bash convention)
        assert_eq!(substitute_args("${@:0}", &args), "one two three");
        // No placeholder -> unchanged
        assert_eq!(substitute_args("plain text", &args), "plain text");
    }

    /// `expandPromptTemplate` must expand `/name args` from disk templates
    /// (frontmatter stripped, args substituted) and leave unknown text alone.
    #[test]
    fn expand_prompt_template_reads_file_and_substitutes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("review.md");
        std::fs::write(
            &file,
            "---\ndescription: Code review\n---\nReview this code: $1\nFocus: $@\n",
        )
        .expect("write");
        let templates = vec![PromptTemplate {
            name: "review".to_string(),
            description: "Code review".to_string(),
            file_path: file.to_string_lossy().to_string(),
            source: PromptSource::User,
            append: false,
            source_info: create_synthetic_source_info(
                file.to_string_lossy().to_string(),
                "local".to_string(),
                None,
                None,
                None,
            ),
        }];

        let expanded = expand_prompt_template("/review src/main.rs", &templates);
        assert_eq!(
            expanded,
            "Review this code: src/main.rs\nFocus: src/main.rs"
        );
        // Unknown template -> original text
        assert_eq!(expand_prompt_template("/nope x", &templates), "/nope x");
        // No leading slash -> original text
        assert_eq!(expand_prompt_template("plain", &templates), "plain");
    }
}
