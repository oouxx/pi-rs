use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;
use super::truncate::{self, TruncationResult};

const DEFAULT_LIMIT: usize = 500;

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

#[derive(Debug, Clone)]
pub struct LsToolOptions {}

impl Default for LsToolOptions {
    fn default() -> Self {
        Self {}
    }
}

fn ls_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Directory to list (default: current directory)" },
            "limit": { "type": "number", "description": "Maximum number of entries to return (default: 500)" }
        }
    })
}

pub fn create_ls_tool(
    cwd: &str,
    _options: Option<LsToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let cwd = cwd.to_string();

    AgentTool {
        name: "ls".to_string(),
        description: "List directory contents. Returns file and directory names.".to_string(),
        label: "Ls".to_string(),
        parameters_schema: ls_parameters_schema(),
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
                    let dir_path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                    let limit = params
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(DEFAULT_LIMIT);

                    let absolute_path = path_utils::resolve_to_cwd(dir_path, &cwd);
                    let absolute_path_str = absolute_path.to_string_lossy().to_string();

                    if !absolute_path.is_dir() {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Not a directory: {}",
                                dir_path
                            ))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    let entries = match tokio::fs::read_dir(&absolute_path_str).await {
                        Ok(mut rd) => {
                            let mut e = Vec::new();
                            while let Ok(Some(entry)) = rd.next_entry().await {
                                if let Some(name) = entry.file_name().to_str() {
                                    e.push(name.to_string());
                                }
                            }
                            e.sort();
                            e
                        }
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

                    let mut details = LsToolDetails::default();
                    let limited_entries: Vec<&String> = entries.iter().take(limit).collect();
                    if entries.len() > limit {
                        details.entry_limit_reached = Some(limit);
                    }

                    let mut output_lines: Vec<String> = Vec::new();
                    for name in &limited_entries {
                        let full_path = absolute_path.join(name);
                        if full_path.is_dir() {
                            output_lines.push(format!("{}/", name));
                        } else {
                            output_lines.push(name.to_string());
                        }
                    }

                    if entries.len() > limit {
                        output_lines.push(format!("... ({} more entries)", entries.len() - limit));
                    }

                    let output = output_lines.join("\n");
                    let truncation = truncate::truncate_head(&output, None);
                    let final_output = if truncation.truncated {
                        details.truncation = Some(truncation.clone());
                        truncation.content
                    } else {
                        output
                    };

                    Ok(AgentToolResult {
                        content: vec![ContentBlock::text(final_output)],
                        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                        terminate: None,
                    })
                })
            },
        ),
    }
}
