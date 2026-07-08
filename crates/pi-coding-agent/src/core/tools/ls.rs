use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;
use super::truncate::{self, TruncationResult};

const DEFAULT_LIMIT: usize = 500;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LsToolInput {
    pub path: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LsToolDetails {
    pub truncation: Option<TruncationResult>,
    pub entry_limit_reached: Option<usize>,
}

// ============================================================================
// LsOperations trait
// ============================================================================

/// Pluggable operations for the ls tool.
/// Override these to delegate directory listing to remote systems (for example SSH).
pub trait LsOperations: Send + Sync {
    /// Check if path exists.
    fn exists(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<bool, Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    >;

    /// Get file or directory metadata. Throws if not found.
    fn is_directory(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<bool, Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    >;

    /// Read directory entries.
    fn read_dir(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    >;
}

// ============================================================================
// LocalLsOperations
// ============================================================================

pub struct LocalLsOperations;

impl LsOperations for LocalLsOperations {
    fn exists(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<bool, Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    > {
        let path = path.to_string();
        Box::pin(async move { Ok(std::path::Path::new(&path).exists()) })
    }

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

    fn read_dir(
        &self,
        path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        let path = path.to_string();
        Box::pin(async move {
            let mut entries = tokio::fs::read_dir(&path).await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            let mut names = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_string());
                }
            }
            Ok(names)
        })
    }
}

// ============================================================================
// LsToolOptions
// ============================================================================

#[derive(Clone)]
pub struct LsToolOptions {
    pub operations: Arc<dyn LsOperations>,
}

impl std::fmt::Debug for LsToolOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LsToolOptions").finish()
    }
}

impl Default for LsToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalLsOperations),
        }
    }
}

// ============================================================================
// Parameters schema
// ============================================================================

fn ls_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Directory to list (default: current directory)" },
            "limit": { "type": "number", "description": "Maximum number of entries to return (default: 500)" }
        }
    })
}

// ============================================================================
// create_ls_tool
// ============================================================================

pub fn create_ls_tool(
    cwd: &str,
    options: Option<LsToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        name: "ls".to_string(),
        description: format!(
            "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. \
             Output is truncated to {} entries or 256KB (whichever is hit first).",
            DEFAULT_LIMIT
        ),
        label: "Ls".to_string(),
        parameters_schema: ls_parameters_schema(),
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
                    let dir_path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                    let limit = params
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(DEFAULT_LIMIT);

                    let absolute_path = path_utils::resolve_to_cwd(dir_path, &cwd);
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

                    // Check if path exists
                    let exists = match operations.exists(&absolute_path_str).await {
                        Ok(e) => e,
                        Err(_) => false,
                    };
                    if !exists {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Path not found: {}",
                                dir_path
                            ))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    // Check if path is a directory
                    let is_dir = match operations.is_directory(&absolute_path_str).await {
                        Ok(d) => d,
                        Err(_) => false,
                    };
                    if !is_dir {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Not a directory: {}",
                                dir_path
                            ))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    // Read directory entries
                    let entries = match operations.read_dir(&absolute_path_str).await {
                        Ok(e) => e,
                        Err(e) => {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!(
                                    "Error reading directory: {}",
                                    e
                                ))],
                                details: serde_json::Value::Null,
                                terminate: None,
                            });
                        }
                    };

                    // Sort case-insensitively
                    let mut sorted = entries;
                    sorted.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

                    // Format entries with directory indicators
                    let mut results: Vec<String> = Vec::new();
                    let mut entry_limit_reached = false;
                    for entry in &sorted {
                        if results.len() >= limit {
                            entry_limit_reached = true;
                            break;
                        }
                        let full_path = std::path::Path::new(&absolute_path_str).join(entry);
                        let suffix = if full_path.is_dir() { "/" } else { "" };
                        results.push(format!("{}{}", entry, suffix));
                    }

                    if results.is_empty() {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text("(empty directory)".to_string())],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    let raw_output = results.join("\n");
                    let truncation = truncate::truncate_head(&raw_output, None);
                    let mut details = LsToolDetails::default();
                    let mut notices: Vec<String> = Vec::new();

                    if entry_limit_reached {
                        notices.push(format!(
                            "{} entries limit reached. Use limit={} for more",
                            limit,
                            limit * 2
                        ));
                        details.entry_limit_reached = Some(limit);
                    }
                    if truncation.truncated {
                        notices.push("256KB limit reached".to_string());
                        details.truncation = Some(truncation.clone());
                    }

                    let mut result_text = if truncation.truncated {
                        truncation.content
                    } else {
                        raw_output
                    };

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
