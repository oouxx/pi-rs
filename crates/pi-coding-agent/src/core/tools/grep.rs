use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;
use super::truncate::{self, TruncationResult, GREP_MAX_LINE_LENGTH};

const DEFAULT_LIMIT: usize = 100;

// ============================================================================
// Types
// ============================================================================

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

// ============================================================================
// GrepOperations trait
// ============================================================================

/// Pluggable operations for the grep tool.
/// Override these to delegate search to remote systems (for example SSH).
pub trait GrepOperations: Send + Sync {
    /// Check if path is a directory. Throws if path does not exist.
    fn is_directory(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<bool, Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    >;

    /// Read file contents.
    fn read_file(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<String, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    >;
}

// ============================================================================
// LocalGrepOperations
// ============================================================================

pub struct LocalGrepOperations;

impl GrepOperations for LocalGrepOperations {
    fn is_directory(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<bool, Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    > {
        let path = path.to_string();
        Box::pin(async move {
            let meta = tokio::fs::metadata(&path).await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(meta.is_dir())
        })
    }

    fn read_file(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<String, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        let path = path.to_string();
        Box::pin(async move {
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
    }
}

// ============================================================================
// GrepToolOptions
// ============================================================================

#[derive(Clone)]
pub struct GrepToolOptions {
    pub operations: Arc<dyn GrepOperations>,
}

impl std::fmt::Debug for GrepToolOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrepToolOptions").finish()
    }
}

impl Default for GrepToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalGrepOperations),
        }
    }
}

// ============================================================================
// Parameters schema
// ============================================================================

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

// ============================================================================
// create_grep_tool
// ============================================================================

pub fn create_grep_tool(
    cwd: &str,
    options: Option<GrepToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        name: "grep".to_string(),
        description: format!(
            "Search for patterns in files using regex or literal matching. \
             Output is truncated to {} matches or 256KB (whichever is hit first). \
             Long lines are truncated to {} chars.",
            DEFAULT_LIMIT, GREP_MAX_LINE_LENGTH
        ),
        label: "Grep".to_string(),
        parameters_schema: grep_parameters_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(
            move |_tool_call_id: String,
                  params: serde_json::Value,
                  signal: Option<tokio::sync::watch::Receiver<bool>>,
                  _on_update: Option<
                Arc<dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync>,
            >| {
                let cwd = cwd.clone();
                let operations = operations.clone();
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

                    // Check for abort
                    if let Some(ref rx) = signal {
                        if *rx.borrow() {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text("Operation aborted")],
                                details: serde_json::Value::Null,
                                terminate: None,
                            });
                        }
                    }

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
                    let mut lines_truncated = false;

                    if absolute_path.is_file() {
                        if let Err(e) = grep_file(
                            &absolute_path_str,
                            &re,
                            context_lines,
                            &mut output_lines,
                            &mut match_count,
                            limit,
                            &mut match_limit_reached,
                            &mut lines_truncated,
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
                            &mut lines_truncated,
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
                    if lines_truncated {
                        details.lines_truncated = Some(true);
                    }

                    let mut result_text = if truncation.truncated {
                        truncation.content.clone()
                    } else {
                        output
                    };

                    // Build notices
                    let mut notices: Vec<String> = Vec::new();
                    if let Some(reached) = match_limit_reached {
                        notices.push(format!(
                            "{} matches limit reached. Use limit={} for more, or refine pattern",
                            reached,
                            reached * 2
                        ));
                    }
                    if truncation.truncated {
                        notices.push("256KB limit reached".to_string());
                    }
                    if lines_truncated {
                        notices.push(format!(
                            "Some lines truncated to {} chars. Use read tool to see full lines",
                            GREP_MAX_LINE_LENGTH
                        ));
                    }
                    if !notices.is_empty() {
                        result_text.push_str(&format!("\n\n[{}]", notices.join(". ")));
                    }

                    Ok(AgentToolResult {
                        content: vec![ContentBlock::text(result_text)],
                        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                        terminate: None,
                    })
                })
            },
        ),
    }
}

// ============================================================================
// Internal helpers
// ============================================================================

fn grep_file(
    file_path: &str,
    re: &regex::Regex,
    context_lines: usize,
    output_lines: &mut Vec<String>,
    match_count: &mut usize,
    limit: usize,
    match_limit_reached: &mut Option<usize>,
    lines_truncated: &mut bool,
) -> Result<(), String> {
    let content =
        std::fs::read_to_string(file_path).map_err(|e| format!("{}: {}", file_path, e))?;
    let lines: Vec<&str> = content.lines().collect();
    let file_name = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(file_path);

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
                    let (truncated_line, was_truncated) = truncate::truncate_line(lines[j], Some(GREP_MAX_LINE_LENGTH));
                    if was_truncated {
                        *lines_truncated = true;
                    }
                    if j == i {
                        output_lines.push(format!("{}:{}: {}", file_name, j + 1, truncated_line));
                    } else {
                        output_lines.push(format!("{}-{}- {}", file_name, j + 1, truncated_line));
                    }
                }
                output_lines.push("--".to_string());
            } else {
                let (truncated_line, was_truncated) = truncate::truncate_line(line, Some(GREP_MAX_LINE_LENGTH));
                if was_truncated {
                    *lines_truncated = true;
                }
                output_lines.push(format!("{}:{}: {}", file_name, i + 1, truncated_line));
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
    lines_truncated: &mut bool,
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
                lines_truncated,
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
                lines_truncated,
            )?;
            if match_limit_reached.is_some() {
                return Ok(());
            }
        }
    }
    Ok(())
}
