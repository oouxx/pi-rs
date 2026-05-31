use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;
use super::truncate::{self, TruncationResult};

const DEFAULT_LIMIT: usize = 1000;

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

#[derive(Debug, Clone)]
pub struct FindToolOptions {}

impl Default for FindToolOptions {
    fn default() -> Self {
        Self {}
    }
}

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

pub fn create_find_tool(cwd: &str, _options: Option<FindToolOptions>) -> AgentTool<serde_json::Value, serde_json::Value> {
    let cwd = cwd.to_string();

    AgentTool {
        name: "find".to_string(),
        description: "Find files matching a glob pattern. Returns matching file paths.".to_string(),
        label: "Find".to_string(),
        parameters_schema: find_parameters_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id: String, params: serde_json::Value, _signal: Option<tokio::sync::watch::Receiver<bool>>, _on_update: Option<Arc<dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync>>| {
            let cwd = cwd.clone();
            Box::pin(async move {
                let pattern = params.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let search_path = params.get("path").and_then(|v| v.as_str());
                let limit = params.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(DEFAULT_LIMIT);

                let search_cwd = match search_path {
                    Some(p) => path_utils::resolve_to_cwd(p, &cwd).to_string_lossy().to_string(),
                    None => cwd.clone(),
                };

                let full_pattern = if std::path::Path::new(pattern).is_absolute() {
                    pattern.to_string()
                } else {
                    format!("{}/{}", search_cwd, pattern)
                };

                let mut results = Vec::new();
                let ignore_dirs = ["node_modules", ".git", "target"];

                if let Ok(paths) = glob::glob_with(
                    &full_pattern,
                    glob::MatchOptions {
                        case_sensitive: true,
                        require_literal_separator: false,
                        require_literal_leading_dot: false,
                    },
                ) {
                    for entry in paths {
                        if results.len() >= limit { break; }
                        if let Ok(path) = entry {
                            let path_str = path.to_string_lossy().to_string();
                            let skip = ignore_dirs.iter().any(|ign| path_str.contains(ign));
                            if !skip { results.push(path_str); }
                        }
                    }
                }

                let mut details = FindToolDetails::default();
                if results.len() >= limit { details.result_limit_reached = Some(limit); }

                let cwd_path = std::path::Path::new(&cwd);
                let display_results: Vec<String> = results.iter().map(|r| {
                    let path = std::path::Path::new(r);
                    path.strip_prefix(cwd_path)
                        .map(|p| format!("./{}", p.display()))
                        .unwrap_or_else(|_| r.clone())
                }).collect();

                let output = display_results.join("\n");
                let truncation = truncate::truncate_head(&output, None);

                let final_output = if truncation.truncated {
                    details.truncation = Some(truncation.clone());
                    truncation.content
                } else {
                    output
                };

                if display_results.is_empty() {
                    return Ok(AgentToolResult {
                        content: vec![ContentBlock::text("No files found matching pattern")],
                        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                        terminate: None,
                    });
                }

                Ok(AgentToolResult {
                    content: vec![ContentBlock::text(final_output)],
                    details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                    terminate: None,
                })
            })
        }),
    }
}