use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;
use super::truncate::{self, TruncationResult};

const DEFAULT_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepToolInput {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub ignore_case: Option<bool>,
    pub literal: Option<bool>,
    pub context: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GrepToolDetails {
    pub truncation: Option<TruncationResult>,
    pub match_limit_reached: Option<usize>,
    pub lines_truncated: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct GrepToolOptions {}

impl Default for GrepToolOptions {
    fn default() -> Self {
        Self {}
    }
}

fn grep_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "Search pattern (regex or literal string)" },
            "path": { "type": "string", "description": "Directory or file to search (default: current directory)" },
            "glob": { "type": "string", "description": "Filter files by glob pattern, e.g. '*.ts'" },
            "ignoreCase": { "type": "boolean", "description": "Case-insensitive search (default: false)" },
            "literal": { "type": "boolean", "description": "Treat pattern as literal string instead of regex (default: false)" },
            "context": { "type": "number", "description": "Number of lines to show before and after each match (default: 0)" },
            "limit": { "type": "number", "description": "Maximum number of matches to return (default: 100)" }
        },
        "required": ["pattern"]
    })
}

pub fn create_grep_tool(
    cwd: &str,
    _options: Option<GrepToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let cwd = cwd.to_string();

    AgentTool {
        name: "grep".to_string(),
        description: "Search for patterns in files using regex or literal matching.".to_string(),
        label: "Grep".to_string(),
        parameters_schema: grep_parameters_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(
            move |_tool_call_id: String,
                  params: serde_json::Value,
                  _signal: Option<tokio::sync::watch::Receiver<bool>>,
                  _on_update: Option<
                Arc<dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync>,
            >| {
                let cwd = cwd.clone();
                Box::pin(async move {
                    let pattern = params.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                    let search_path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                    let glob_pattern = params.get("glob").and_then(|v| v.as_str());
                    let ignore_case = params
                        .get("ignoreCase")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let literal = params
                        .get("literal")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let context_lines = params
                        .get("context")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(0);
                    let limit = params
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(DEFAULT_LIMIT);

                    let absolute_path = path_utils::resolve_to_cwd(search_path, &cwd);
                    let absolute_path_str = absolute_path.to_string_lossy().to_string();

                    let regex_pattern = if literal {
                        regex::escape(pattern)
                    } else {
                        pattern.to_string()
                    };
                    let re = match regex::RegexBuilder::new(&regex_pattern)
                        .case_insensitive(ignore_case)
                        .build()
                    {
                        Ok(re) => re,
                        Err(e) => {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!(
                                    "Invalid regex pattern: {}",
                                    e
                                ))],
                                details: serde_json::Value::Null,
                                terminate: None,
                            });
                        }
                    };

                    let mut output_lines: Vec<String> = Vec::new();
                    let mut match_count = 0;
                    let mut match_limit_reached = None;

                    if absolute_path.is_file() {
                        if let Err(e) = grep_file(
                            &absolute_path_str,
                            &re,
                            context_lines,
                            &mut output_lines,
                            &mut match_count,
                            limit,
                            &mut match_limit_reached,
                        ) {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!(
                                    "Error searching file: {}",
                                    e
                                ))],
                                details: serde_json::Value::Null,
                                terminate: None,
                            });
                        }
                    } else if absolute_path.is_dir() {
                        let glob_matcher = glob_pattern.and_then(|g| glob::Pattern::new(g).ok());
                        if let Err(e) = search_directory(
                            &absolute_path,
                            &re,
                            &glob_matcher,
                            context_lines,
                            &mut output_lines,
                            &mut match_count,
                            limit,
                            &mut match_limit_reached,
                        ) {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!(
                                    "Error searching directory: {}",
                                    e
                                ))],
                                details: serde_json::Value::Null,
                                terminate: None,
                            });
                        }
                    } else {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Path not found: {}",
                                search_path
                            ))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    if output_lines.is_empty() {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text("No matches found")],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    let output = output_lines.join("\n");
                    let truncation = truncate::truncate_head(&output, None);
                    let mut details = GrepToolDetails::default();
                    if truncation.truncated {
                        details.truncation = Some(truncation.clone());
                    }
                    if let Some(reached) = match_limit_reached {
                        details.match_limit_reached = Some(reached);
                    }

                    Ok(AgentToolResult {
                        content: vec![ContentBlock::text(if truncation.truncated {
                            truncation.content
                        } else {
                            output
                        })],
                        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                        terminate: None,
                    })
                })
            },
        ),
    }
}

fn grep_file(
    file_path: &str,
    re: &regex::Regex,
    context_lines: usize,
    output_lines: &mut Vec<String>,
    match_count: &mut usize,
    limit: usize,
    match_limit_reached: &mut Option<usize>,
) -> Result<(), String> {
    let content =
        std::fs::read_to_string(file_path).map_err(|e| format!("{}: {}", file_path, e))?;
    let lines: Vec<&str> = content.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        if *match_count >= limit {
            *match_limit_reached = Some(limit);
            return Ok(());
        }
        if re.is_match(line) {
            *match_count += 1;
            if context_lines > 0 {
                let start = if i >= context_lines {
                    i - context_lines
                } else {
                    0
                };
                let end = std::cmp::min(i + context_lines + 1, lines.len());
                for j in start..end {
                    let prefix = if j == i { ">" } else { " " };
                    let (truncated_line, _) = truncate::truncate_line(lines[j], Some(500));
                    output_lines.push(format!("{}{}:{}", prefix, file_path, j + 1));
                    output_lines.push(format!("  {}", truncated_line));
                }
                output_lines.push("--".to_string());
            } else {
                let (truncated_line, _) = truncate::truncate_line(line, Some(500));
                output_lines.push(format!("{}:{}:{}", file_path, i + 1, truncated_line));
            }
        }
    }
    Ok(())
}

fn search_directory(
    dir: &std::path::Path,
    re: &regex::Regex,
    glob_matcher: &Option<glob::Pattern>,
    context_lines: usize,
    output_lines: &mut Vec<String>,
    match_count: &mut usize,
    limit: usize,
    match_limit_reached: &mut Option<usize>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if dir_name.starts_with('.') || dir_name == "node_modules" || dir_name == "target" {
                continue;
            }
            search_directory(
                &path,
                re,
                glob_matcher,
                context_lines,
                output_lines,
                match_count,
                limit,
                match_limit_reached,
            )?;
            if match_limit_reached.is_some() {
                return Ok(());
            }
        } else if path.is_file() {
            if let Some(ref matcher) = glob_matcher {
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !matcher.matches(file_name) {
                    continue;
                }
            }
            let path_str = path.to_string_lossy().to_string();
            grep_file(
                &path_str,
                re,
                context_lines,
                output_lines,
                match_count,
                limit,
                match_limit_reached,
            )?;
            if match_limit_reached.is_some() {
                return Ok(());
            }
        }
    }
    Ok(())
}
