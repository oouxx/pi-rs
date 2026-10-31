//! Grep tool — port of `packages/coding-agent/src/core/tools/grep.ts`.
//!
//! Searches with the `ripgrep` binary (`rg`), resolved through
//! [`crate::utils::tools_manager::ensure_tool`]: managed install in the pi bin
//! directory first, then `rg` on PATH, then a GitHub release download. This
//! matches TS, which also shells out to `rg` and reports an error when the
//! binary is unavailable.
//!
//! `rg --json` streams one JSON object per line; matches are collected while
//! streaming (so the child can be killed as soon as the match limit is hit) and
//! formatted afterwards, because the context-line formatter needs async file
//! reads through [`GrepOperations`].

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

use super::path_utils;
use super::truncate::{
    self, truncate_head_bytes_only, TruncationResult, DEFAULT_MAX_BYTES, GREP_MAX_LINE_LENGTH,
};
use super::wait_for_abort;
use crate::utils::tools_manager;

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

/// One `{"type":"match", ...}` event from `rg --json`.
#[derive(Debug, Clone)]
struct GrepMatch {
    file_path: String,
    line_number: usize,
    line_text: Option<String>,
}

// ============================================================================
// GrepOperations trait
// ============================================================================

/// Pluggable operations for the grep tool.
/// Override these to delegate search to remote systems (for example SSH).
///
/// Matching TS `GrepOperations`: only filesystem access is pluggable; the
/// search itself always runs through `rg`.
pub trait GrepOperations: Send + Sync {
    /// Check if path is a directory. Throws if path does not exist.
    fn is_directory(&self, path: &str) -> crate::core::tools::AsyncOpResult<bool>;

    /// Read file contents.
    fn read_file(&self, path: &str) -> crate::core::tools::AsyncOpResult<String>;
}

// ============================================================================
// LocalGrepOperations
// ============================================================================

pub struct LocalGrepOperations;

impl GrepOperations for LocalGrepOperations {
    fn is_directory(&self, path: &str) -> crate::core::tools::AsyncOpResult<bool> {
        let path = path.to_string();
        Box::pin(async move {
            let meta = tokio::fs::metadata(&path)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(meta.is_dir())
        })
    }

    fn read_file(&self, path: &str) -> crate::core::tools::AsyncOpResult<String> {
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
            "glob": { "type": "string", "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'" },
            "ignoreCase": { "type": "boolean", "description": "Case-insensitive search (default: false)" },
            "literal": { "type": "boolean", "description": "Treat pattern as literal string instead of regex (default: false)" },
            "context": { "type": "number", "description": "Number of lines to show before and after each match (default: 0)" },
            "limit": { "type": "number", "description": "Maximum number of matches to return (default: 100)" }
        },
        "required": ["pattern"]
    })
}

// ============================================================================
// Arguments
// ============================================================================

/// Build the `rg` argument vector, matching TS byte for byte.
fn build_rg_args(
    pattern: &str,
    search_path: &str,
    glob: Option<&str>,
    ignore_case: bool,
    literal: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--json".to_string(),
        "--line-number".to_string(),
        "--color=never".to_string(),
        "--hidden".to_string(),
    ];
    if ignore_case {
        args.push("--ignore-case".to_string());
    }
    if literal {
        args.push("--fixed-strings".to_string());
    }
    if let Some(glob) = glob {
        args.push("--glob".to_string());
        args.push(glob.to_string());
    }
    args.push("--".to_string());
    args.push(pattern.to_string());
    args.push(search_path.to_string());
    args
}

// ============================================================================
// Output formatting
// ============================================================================

/// TS `formatPath()`: relative to the search root when searching a directory
/// (unless the match lives outside it), otherwise the basename.
fn format_path(file_path: &str, search_path: &str, is_directory: bool) -> String {
    if is_directory {
        if let Ok(relative) = std::path::Path::new(file_path).strip_prefix(search_path) {
            let relative = relative.to_string_lossy().replace('\\', "/");
            if !relative.is_empty() && !relative.starts_with("..") {
                return relative;
            }
        }
    }
    std::path::Path::new(file_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string())
}

/// Normalize file content into lines the way TS does
/// (`replace(/\r\n/g, "\n").replace(/\r/g, "\n").split("\n")`).
fn split_file_lines(content: &str) -> Vec<String> {
    content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect()
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
        constrained_sampling: None,
        name: "grep".to_string(),
        description: format!(
            "Search file contents for a pattern. Returns matching lines with file paths and line numbers. \
             Respects .gitignore. Output is truncated to {} matches or {}KB (whichever is hit first). \
             Long lines are truncated to {} chars.",
            DEFAULT_LIMIT,
            DEFAULT_MAX_BYTES / 1024,
            GREP_MAX_LINE_LENGTH
        ),
        label: "grep".to_string(),
        prompt_snippet: Some("Search file contents for patterns (respects .gitignore)".to_string()),
        prompt_guidelines: None,
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
                    let search_dir = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                    let glob = params.get("glob").and_then(|v| v.as_str());
                    let ignore_case = params
                        .get("ignoreCase")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let literal = params
                        .get("literal")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let context_value = params
                        .get("context")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let effective_limit = params
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(DEFAULT_LIMIT)
                        .max(1);

                    // Check for abort
                    if let Some(ref rx) = signal {
                        if *rx.borrow() {
                            return Err(abort_error());
                        }
                    }

                    let search_path = path_utils::resolve_to_cwd(search_dir, &cwd)
                        .to_string_lossy()
                        .to_string();

                    let is_directory = match operations.is_directory(&search_path).await {
                        Ok(is_dir) => is_dir,
                        Err(_) => {
                            return Err(format!("Path not found: {}", search_path).into());
                        }
                    };

                    let rg_path = match tools_manager::ensure_tool_async("rg").await {
                        Some(path) => path,
                        None => {
                            return Err(
                                "ripgrep (rg) is not available and could not be downloaded"
                                    .to_string()
                                    .into(),
                            );
                        }
                    };

                    let args = build_rg_args(
                        pattern,
                        &search_path,
                        glob,
                        ignore_case,
                        literal,
                    );

                    let mut child = tokio::process::Command::new(&rg_path)
                        .args(&args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .map_err(|e| format!("Failed to run ripgrep: {}", e))?;

                    let stdout = match child.stdout.take() {
                        Some(stdout) => stdout,
                        None => {
                            return Err("Failed to run ripgrep: stdout unavailable"
                                .to_string()
                                .into());
                        }
                    };
                    let stderr_pipe = child.stderr.take();
                    let stderr_task = tokio::spawn(async move {
                        let mut buffer = String::new();
                        if let Some(mut stderr) = stderr_pipe {
                            let _ = stderr.read_to_string(&mut buffer).await;
                        }
                        buffer
                    });

                    // Stream `--json` events; kill the child on abort or as soon
                    // as the match limit is reached (TS `stopChild`).
                    let mut lines = tokio::io::BufReader::new(stdout).lines();
                    let mut matches: Vec<GrepMatch> = Vec::new();
                    let mut match_count = 0usize;
                    let mut match_limit_reached = false;
                    let mut aborted = false;
                    let mut killed_due_to_limit = false;
                    let mut signal_rx = signal.clone();

                    loop {
                        let next_line = match signal_rx.as_mut() {
                            Some(rx) => {
                                tokio::select! {
                                    line = lines.next_line() => line,
                                    () = wait_for_abort(rx) => {
                                        aborted = true;
                                        let _ = child.kill().await;
                                        break;
                                    }
                                }
                            }
                            None => lines.next_line().await,
                        };
                        match next_line {
                            Ok(Some(line)) => {
                                if line.trim().is_empty() {
                                    continue;
                                }
                                let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
                                    continue;
                                };
                                if event.get("type").and_then(|v| v.as_str()) != Some("match") {
                                    continue;
                                }
                                let data = event.get("data");
                                let file_path = data
                                    .and_then(|d| d.get("path"))
                                    .and_then(|p| p.get("text"))
                                    .and_then(|t| t.as_str());
                                let line_number = data
                                    .and_then(|d| d.get("line_number"))
                                    .and_then(|n| n.as_u64());
                                let line_text = data
                                    .and_then(|d| d.get("lines"))
                                    .and_then(|l| l.get("text"))
                                    .and_then(|t| t.as_str());
                                match_count += 1;
                                if let (Some(file_path), Some(line_number)) = (file_path, line_number) {
                                    matches.push(GrepMatch {
                                        file_path: file_path.to_string(),
                                        line_number: line_number as usize,
                                        line_text: line_text.map(str::to_string),
                                    });
                                }
                                if match_count >= effective_limit {
                                    match_limit_reached = true;
                                    killed_due_to_limit = true;
                                    let _ = child.kill().await;
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }

                    let status = child.wait().await.ok();
                    let stderr = stderr_task.await.unwrap_or_default();

                    if aborted {
                        return Err(abort_error());
                    }
                    if !killed_due_to_limit {
                        let code = status.and_then(|s| s.code());
                        if code != Some(0) && code != Some(1) {
                            let message = stderr.trim();
                            if message.is_empty() {
                                return Err(format!(
                                    "ripgrep exited with code {}",
                                    code.map_or_else(|| "unknown".to_string(), |c| c.to_string())
                                )
                                .into());
                            }
                            return Err(message.to_string().into());
                        }
                    }
                    if match_count == 0 {
                        return Ok(text_result("No matches found"));
                    }

                    // Format after streaming so custom readFile() backends can
                    // be async (matching TS).
                    let mut output_lines: Vec<String> = Vec::new();
                    let mut lines_truncated = false;
                    let mut file_cache: HashMap<String, Vec<String>> = HashMap::new();

                    for found in &matches {
                        let relative_path =
                            format_path(&found.file_path, &search_path, is_directory);
                        if context_value == 0 {
                            if let Some(ref line_text) = found.line_text {
                                let sanitized =
                                    line_text.replace("\r\n", "\n").replace('\r', "");
                                let sanitized = sanitized.strip_suffix('\n').unwrap_or(&sanitized);
                                let (text, was_truncated) = truncate::truncate_line(
                                    sanitized,
                                    Some(GREP_MAX_LINE_LENGTH),
                                );
                                if was_truncated {
                                    lines_truncated = true;
                                }
                                output_lines.push(format!(
                                    "{}:{}: {}",
                                    relative_path, found.line_number, text
                                ));
                                continue;
                            }
                        }

                        let file_lines = if let Some(cached) = file_cache.get(&found.file_path) {
                            cached.clone()
                        } else {
                            let loaded = match operations.read_file(&found.file_path).await {
                                Ok(content) => split_file_lines(&content),
                                Err(_) => Vec::new(),
                            };
                            file_cache.insert(found.file_path.clone(), loaded.clone());
                            loaded
                        };

                        if file_lines.is_empty() {
                            output_lines.push(format!(
                                "{}:{}: (unable to read file)",
                                relative_path, found.line_number
                            ));
                            continue;
                        }

                        let start = if context_value > 0 {
                            found.line_number.saturating_sub(context_value).max(1)
                        } else {
                            found.line_number
                        };
                        let end = if context_value > 0 {
                            std::cmp::min(
                                file_lines.len(),
                                found.line_number.saturating_add(context_value),
                            )
                        } else {
                            found.line_number
                        };

                        for current in start..=end {
                            let line_text = file_lines
                                .get(current.saturating_sub(1))
                                .map(String::as_str)
                                .unwrap_or("");
                            let sanitized = line_text.replace('\r', "");
                            let (text, was_truncated) = truncate::truncate_line(
                                &sanitized,
                                Some(GREP_MAX_LINE_LENGTH),
                            );
                            if was_truncated {
                                lines_truncated = true;
                            }
                            if current == found.line_number {
                                output_lines.push(format!(
                                    "{}:{}: {}",
                                    relative_path, current, text
                                ));
                            } else {
                                output_lines.push(format!(
                                    "{}-{}- {}",
                                    relative_path, current, text
                                ));
                            }
                        }
                    }

                    let raw_output = output_lines.join("\n");
                    let truncation = truncate_head_bytes_only(&raw_output);
                    let mut output = truncation.content.clone();
                    let mut details = GrepToolDetails::default();
                    let mut notices: Vec<String> = Vec::new();

                    if match_limit_reached {
                        notices.push(format!(
                            "{} matches limit reached. Use limit={} for more, or refine pattern",
                            effective_limit,
                            effective_limit * 2
                        ));
                        details.match_limit_reached = Some(effective_limit);
                    }
                    if truncation.truncated {
                        notices.push(format!(
                            "{} limit reached",
                            truncate::format_size(DEFAULT_MAX_BYTES)
                        ));
                        details.truncation = Some(truncation.clone());
                    }
                    if lines_truncated {
                        notices.push(format!(
                            "Some lines truncated to {} chars. Use read tool to see full lines",
                            GREP_MAX_LINE_LENGTH
                        ));
                        details.lines_truncated = Some(true);
                    }
                    if !notices.is_empty() {
                        output.push_str(&format!("\n\n[{}]", notices.join(". ")));
                    }

                    Ok(AgentToolResult {
                        content: vec![ContentBlock::text(output)],
                        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                        usage: None,
                        terminate: None,
                    })
                })
            },
        ),
    }
}

/// TS rejects with `new Error("Operation aborted")` on abort.
fn abort_error() -> Box<dyn std::error::Error + Send + Sync> {
    "Operation aborted".to_string().into()
}

fn text_result(text: &str) -> AgentToolResult<serde_json::Value> {
    AgentToolResult {
        content: vec![ContentBlock::text(text)],
        details: serde_json::Value::Null,
        usage: None,
        terminate: None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_grep_tool_input_deserialization() {
        let json = serde_json::json!({
            "pattern": "hello",
            "path": "/tmp",
            "context": 2,
            "limit": 10
        });
        let input: GrepToolInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.pattern, "hello");
        assert_eq!(input.path, Some("/tmp".to_string()));
        assert_eq!(input.context, Some(2));
        assert_eq!(input.limit, Some(10));
    }

    #[test]
    fn test_grep_tool_input_minimal() {
        let json = serde_json::json!({
            "pattern": "hello"
        });
        let input: GrepToolInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.pattern, "hello");
        assert_eq!(input.path, None);
    }

    #[test]
    fn test_grep_parameters_schema() {
        let schema = grep_parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["pattern"].is_object());
        assert!(schema["properties"]["path"].is_object());
        assert_eq!(schema["required"][0], "pattern");
    }

    #[test]
    fn test_grep_tool_creation() {
        let tool = create_grep_tool("/tmp", None);
        assert_eq!(tool.name, "grep");
        assert!(!tool.description.is_empty());
        assert!(tool.parameters_schema.is_object());
    }

    /// Arguments must match TS `createGrepToolDefinition` exactly.
    #[test]
    fn test_build_rg_args() {
        let args = build_rg_args("needle", "/tmp/dir", Some("*.ts"), true, true);
        assert_eq!(
            args,
            vec![
                "--json",
                "--line-number",
                "--color=never",
                "--hidden",
                "--ignore-case",
                "--fixed-strings",
                "--glob",
                "*.ts",
                "--",
                "needle",
                "/tmp/dir",
            ]
        );

        let minimal = build_rg_args("needle", "/tmp", None, false, false);
        assert_eq!(
            minimal,
            vec![
                "--json",
                "--line-number",
                "--color=never",
                "--hidden",
                "--",
                "needle",
                "/tmp"
            ]
        );
    }

    #[test]
    fn test_format_path() {
        assert_eq!(format_path("/tmp/dir/a.txt", "/tmp/dir", true), "a.txt");
        assert_eq!(
            format_path("/tmp/dir/sub/a.txt", "/tmp/dir", true),
            "sub/a.txt"
        );
        // Outside the search root -> basename (TS `!relative.startsWith("..")`).
        assert_eq!(format_path("/tmp/other/a.txt", "/tmp/dir", true), "a.txt");
        // File search (isDirectory=false) -> basename.
        assert_eq!(
            format_path("/tmp/dir/a.txt", "/tmp/dir/a.txt", false),
            "a.txt"
        );
    }
}
