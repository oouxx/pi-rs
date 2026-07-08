use std::fmt;
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;
use super::truncate::{self, format_size, TruncationResult, DEFAULT_MAX_BYTES};

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadToolInput {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReadToolDetails {
    pub truncation: Option<TruncationResult>,
}

// ============================================================================
// ReadOperations trait
// ============================================================================

/// Pluggable operations for the read tool.
/// Override these to delegate file reading to remote systems (for example SSH).
pub trait ReadOperations: Send + Sync {
    fn read_file(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    >;

    /// Check if file is readable (throw if not).
    fn access(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    >;
}

// ============================================================================
// LocalReadOperations
// ============================================================================

pub struct LocalReadOperations;

impl ReadOperations for LocalReadOperations {
    fn read_file(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        let path = path.to_string();
        Box::pin(async move {
            tokio::fs::read(&path)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
    }

    fn access(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    > {
        let path = path.to_string();
        Box::pin(async move {
            tokio::fs::metadata(&path)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(())
        })
    }
}

// ============================================================================
// ReadToolOptions
// ============================================================================

#[derive(Clone)]
pub struct ReadToolOptions {
    pub operations: Arc<dyn ReadOperations>,
}

impl fmt::Debug for ReadToolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadToolOptions").finish()
    }
}

impl Default for ReadToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalReadOperations),
        }
    }
}

// ============================================================================
// Parameters schema
// ============================================================================

fn read_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the file to read (relative or absolute)"
            },
            "offset": {
                "type": "number",
                "description": "Line number to start reading from (1-indexed)"
            },
            "limit": {
                "type": "number",
                "description": "Maximum number of lines to read"
            }
        },
        "required": ["path"]
    })
}

// ============================================================================
// create_read_tool
// ============================================================================

pub fn create_read_tool(
    cwd: &str,
    options: Option<ReadToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        name: "read".to_string(),
        description: format!(
            "Read the contents of a file. Returns the file content with line numbers. \
             Output is truncated to {} lines or {}KB (whichever is hit first). \
             Use offset/limit for large files.",
            truncate::DEFAULT_MAX_LINES,
            DEFAULT_MAX_BYTES / 1024
        ),
        label: "Read".to_string(),
        parameters_schema: read_parameters_schema(),
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
                    let file_path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let offset = params
                        .get("offset")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize);
                    let limit = params
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize);

                    let absolute_path = path_utils::resolve_read_path(file_path, &cwd);
                    let absolute_path_str = absolute_path.to_string_lossy().to_string();

                    // Check if file exists and is readable
                    if let Err(e) = operations.access(&absolute_path_str).await {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Error reading file: {}",
                                e
                            ))],
                            details: serde_json::to_value(ReadToolDetails::default())
                                .unwrap_or(serde_json::Value::Null),
                            terminate: None,
                        });
                    }

                    // Check for abort
                    if let Some(ref rx) = signal {
                        if *rx.borrow() {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text("Operation aborted")],
                                details: serde_json::to_value(ReadToolDetails::default())
                                    .unwrap_or(serde_json::Value::Null),
                                terminate: None,
                            });
                        }
                    }

                    let bytes = match operations.read_file(&absolute_path_str).await {
                        Ok(b) => b,
                        Err(e) => {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!(
                                    "Error reading file: {}",
                                    e
                                ))],
                                details: serde_json::to_value(ReadToolDetails::default())
                                    .unwrap_or(serde_json::Value::Null),
                                terminate: None,
                            });
                        }
                    };

                    let text_content = match String::from_utf8(bytes) {
                        Ok(s) => s,
                        Err(_) => {
                            let file_size = absolute_path
                                .metadata()
                                .map(|m| m.len() as usize)
                                .unwrap_or(0);
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!(
                                    "Binary file: {} ({})",
                                    file_path,
                                    format_size(file_size)
                                ))],
                                details: serde_json::to_value(ReadToolDetails::default())
                                    .unwrap_or(serde_json::Value::Null),
                                terminate: None,
                            });
                        }
                    };

                    let all_lines: Vec<&str> = text_content.split('\n').collect();
                    let total_file_lines = all_lines.len();
                    let start_line = offset.map(|o| if o > 0 { o - 1 } else { 0 }).unwrap_or(0);

                    if start_line >= all_lines.len() {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Offset {} is beyond end of file ({} lines total)",
                                offset.unwrap_or(0),
                                total_file_lines
                            ))],
                            details: serde_json::to_value(ReadToolDetails::default())
                                .unwrap_or(serde_json::Value::Null),
                            terminate: None,
                        });
                    }

                    let selected_content = if let Some(lim) = limit {
                        let end_line = std::cmp::min(start_line + lim, all_lines.len());
                        all_lines[start_line..end_line].join("\n")
                    } else {
                        all_lines[start_line..].join("\n")
                    };

                    let user_limited_lines =
                        limit.map(|lim| std::cmp::min(lim, all_lines.len() - start_line));
                    let truncation = truncate::truncate_head(&selected_content, None);
                    let mut details = ReadToolDetails::default();
                    let output_text;

                    if truncation.first_line_exceeds_limit {
                        let first_line_size = format_size(all_lines[start_line].len());
                        output_text = format!(
                            "[Line {} is {}, exceeds {} limit. Use bash: sed -n '{}p' {} | head -c {}]",
                            start_line + 1, first_line_size, format_size(DEFAULT_MAX_BYTES),
                            start_line + 1, file_path, DEFAULT_MAX_BYTES
                        );
                        details.truncation = Some(truncation);
                    } else if truncation.truncated {
                        let end_line_display = start_line + truncation.output_lines;
                        let next_offset = end_line_display + 1;
                        let truncated_by = truncation.truncated_by.as_deref().unwrap_or("lines");
                        output_text = if truncated_by == "lines" {
                            format!(
                                "{}\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                                truncation.content,
                                start_line + 1,
                                end_line_display,
                                total_file_lines,
                                next_offset
                            )
                        } else {
                            format!(
                                "{}\n\n[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                                truncation.content, start_line + 1, end_line_display, total_file_lines,
                                format_size(DEFAULT_MAX_BYTES), next_offset
                            )
                        };
                        details.truncation = Some(truncation);
                    } else if let Some(ull) = user_limited_lines {
                        if start_line + ull < all_lines.len() {
                            let remaining = all_lines.len() - (start_line + ull);
                            let next_offset = start_line + ull + 1;
                            output_text = format!(
                                "{}\n\n[{} more lines in file. Use offset={} to continue.]",
                                truncation.content, remaining, next_offset
                            );
                        } else {
                            output_text = truncation.content;
                        }
                    } else {
                        output_text = truncation.content;
                    }

                    Ok(AgentToolResult {
                        content: vec![ContentBlock::text(output_text)],
                        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                        terminate: None,
                    })
                })
            },
        ),
    }
}
