use std::fmt;
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteToolInput {
    pub path: String,
    pub content: String,
}

pub trait WriteOperations: Send + Sync {
    fn write_file(
        &self,
        path: &str,
        content: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>>;

    fn mkdir(
        &self,
        dir: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>>;
}

pub struct LocalWriteOperations;

impl WriteOperations for LocalWriteOperations {
    fn write_file(
        &self,
        path: &str,
        content: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>> {
        let path = path.to_string();
        let content = content.to_string();
        Box::pin(async move {
            tokio::fs::write(&path, &content)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
    }

    fn mkdir(
        &self,
        dir: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>> {
        let dir = dir.to_string();
        Box::pin(async move {
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
    }
}

#[derive(Clone)]
pub struct WriteToolOptions {
    pub operations: Arc<dyn WriteOperations>,
}

impl fmt::Debug for WriteToolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteToolOptions").finish()
    }
}

impl Default for WriteToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalWriteOperations),
        }
    }
}

fn write_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the file to write (relative or absolute)"
            },
            "content": {
                "type": "string",
                "description": "Content to write to the file"
            }
        },
        "required": ["path", "content"]
    })
}

pub fn create_write_tool(cwd: &str, options: Option<WriteToolOptions>) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        name: "write".to_string(),
        description: "Write content to a file. Creates the file and any parent directories if they don't exist.".to_string(),
        label: "Write".to_string(),
        parameters_schema: write_parameters_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id: String, params: serde_json::Value, _signal: Option<tokio::sync::watch::Receiver<bool>>, _on_update: Option<Arc<dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync>>| {
            let cwd = cwd.clone();
            let operations = operations.clone();
            Box::pin(async move {
                let file_path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");

                let absolute_path = path_utils::resolve_to_cwd(file_path, &cwd);
                let absolute_path_str = absolute_path.to_string_lossy().to_string();

                if let Some(parent) = absolute_path.parent() {
                    let parent_str = parent.to_string_lossy().to_string();
                    if !parent.exists() {
                        if let Err(e) = operations.mkdir(&parent_str).await {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!("Error creating directory: {}", e))],
                                details: serde_json::Value::Null,
                                terminate: None,
                            });
                        }
                    }
                }

                match operations.write_file(&absolute_path_str, content).await {
                    Ok(()) => {
                        let line_count = content.lines().count();
                        Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Successfully wrote {} lines to {}",
                                line_count, file_path
                            ))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        })
                    }
                    Err(e) => Ok(AgentToolResult {
                        content: vec![ContentBlock::text(format!("Error writing file: {}", e))],
                        details: serde_json::Value::Null,
                        terminate: None,
                    }),
                }
            })
        }),
    }
}