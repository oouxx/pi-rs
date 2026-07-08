use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;
use super::truncate::{self, TruncationResult};

const DEFAULT_LIMIT: usize = 1000;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindToolInput {
    pub pattern: String,
    pub path: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FindToolDetails {
    pub truncation: Option<TruncationResult>,
    pub result_limit_reached: Option<usize>,
}

// ============================================================================
// FindOperations trait
// ============================================================================

/// Pluggable operations for the find tool.
/// Override these to delegate file search to remote systems (for example SSH).
pub trait FindOperations: Send + Sync {
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

    /// Find files matching glob pattern. Returns paths.
    fn glob(
        &self,
        pattern: &str,
        cwd: &str,
        ignore: &[String],
        limit: usize,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    >;
}

// ============================================================================
// LocalFindOperations
// ============================================================================

pub struct LocalFindOperations;

impl FindOperations for LocalFindOperations {
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

    fn glob(
        &self,
        pattern: &str,
        cwd: &str,
        ignore: &[String],
        limit: usize,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        let pattern = pattern.to_string();
        let cwd = cwd.to_string();
        let ignore = ignore.to_vec();
        Box::pin(async move {
            let full_pattern = if std::path::Path::new(&pattern).is_absolute() {
                pattern.clone()
            } else {
                format!("{}/{}", cwd, pattern)
            };

            let mut results = Vec::new();
            if let Ok(paths) = glob::glob_with(
                &full_pattern,
                glob::MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: false,
                    require_literal_leading_dot: false,
                },
            ) {
                for entry in paths {
                    if results.len() >= limit {
                        break;
                    }
                    if let Ok(path) = entry {
                        let path_str = path.to_string_lossy().to_string();
                        let skip = ignore.iter().any(|ign| path_str.contains(ign));
                        if !skip {
                            results.push(path_str);
                        }
                    }
                }
            }
            Ok(results)
        })
    }
}

// ============================================================================
// FindToolOptions
// ============================================================================

#[derive(Clone)]
pub struct FindToolOptions {
    pub operations: Arc<dyn FindOperations>,
}

impl std::fmt::Debug for FindToolOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FindToolOptions").finish()
    }
}

impl Default for FindToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalFindOperations),
        }
    }
}

// ============================================================================
// Parameters schema
// ============================================================================

fn find_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json'" },
            "path": { "type": "string", "description": "Directory to search in (default: current directory)" },
            "limit": { "type": "number", "description": "Maximum number of results (default: 1000)" }
        },
        "required": ["pattern"]
    })
}

// ============================================================================
// create_find_tool
// ============================================================================

pub fn create_find_tool(
    cwd: &str,
    options: Option<FindToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        name: "find".to_string(),
        description: format!(
            "Find files matching a glob pattern. Returns matching file paths relative to the search directory. \
             Output is truncated to {} results or 256KB (whichever is hit first).",
            DEFAULT_LIMIT
        ),
        label: "Find".to_string(),
        parameters_schema: find_parameters_schema(),
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
                    let search_path = params.get("path").and_then(|v| v.as_str());
                    let limit = params
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(DEFAULT_LIMIT);

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

                    let search_cwd = match search_path {
                        Some(p) => path_utils::resolve_to_cwd(p, &cwd)
                            .to_string_lossy()
                            .to_string(),
                        None => cwd.clone(),
                    };

                    // Check if search path exists
                    if !std::path::Path::new(&search_cwd).exists() {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Path not found: {}",
                                search_path.unwrap_or(".")
                            ))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    let ignore_dirs = [
                        "**/node_modules/**".to_string(),
                        "**/.git/**".to_string(),
                    ];

                    let results = match operations.glob(pattern, &search_cwd, &ignore_dirs, limit).await {
                        Ok(r) => r,
                        Err(e) => {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!(
                                    "Error searching: {}",
                                    e
                                ))],
                                details: serde_json::Value::Null,
                                terminate: None,
                            });
                        }
                    };

                    if results.is_empty() {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text("No files found matching pattern")],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    // Relativize paths
                    let cwd_path = std::path::Path::new(&search_cwd);
                    let relativized: Vec<String> = results
                        .iter()
                        .map(|r| {
                            let path = std::path::Path::new(r);
                            path.strip_prefix(cwd_path)
                                .map(|p| format!("./{}", p.display()))
                                .unwrap_or_else(|_| r.clone())
                        })
                        .collect();

                    let result_limit_reached = relativized.len() >= limit;
                    let raw_output = relativized.join("\n");
                    let truncation = truncate::truncate_head(&raw_output, None);
                    let mut details = FindToolDetails::default();
                    let mut notices: Vec<String> = Vec::new();

                    if result_limit_reached {
                        notices.push(format!(
                            "{} results limit reached. Use limit={} for more, or refine pattern",
                            limit,
                            limit * 2
                        ));
                        details.result_limit_reached = Some(limit);
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
