use std::fmt;
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceEdit {
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditToolInput {
    pub path: String,
    pub edits: Vec<ReplaceEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EditToolDetails {}

pub trait EditOperations: Send + Sync {
    fn read_file(
        &self,
        path: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, Box<dyn std::error::Error + Send + Sync>>> + Send>>;

    fn write_file(
        &self,
        path: &str,
        content: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>>;
}

pub struct LocalEditOperations;

impl EditOperations for LocalEditOperations {
    fn read_file(
        &self,
        path: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, Box<dyn std::error::Error + Send + Sync>>> + Send>> {
        let path = path.to_string();
        Box::pin(async move {
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
    }

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
}

#[derive(Clone)]
pub struct EditToolOptions {
    pub operations: Arc<dyn EditOperations>,
}

impl fmt::Debug for EditToolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditToolOptions").finish()
    }
}

impl Default for EditToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalEditOperations),
        }
    }
}

fn edit_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the file to edit (relative or absolute)"
            },
            "edits": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "oldText": {
                            "type": "string",
                            "description": "Exact text for one targeted replacement."
                        },
                        "newText": {
                            "type": "string",
                            "description": "Replacement text for this targeted edit."
                        }
                    },
                    "required": ["oldText", "newText"],
                    "additionalProperties": false
                },
                "description": "One or more targeted replacements."
            }
        },
        "required": ["path", "edits"],
        "additionalProperties": false
    })
}

fn apply_edits(content: &str, edits: &[ReplaceEdit]) -> Result<String, String> {
    let mut result = content.to_string();
    for edit in edits {
        let count = result.matches(&edit.old_text).count();
        if count == 0 {
            return Err(format!(
                "oldText not found in file: {}",
                if edit.old_text.len() > 100 { format!("{}...", &edit.old_text[..100]) } else { edit.old_text.clone() }
            ));
        }
        if count > 1 {
            return Err(format!(
                "oldText is not unique in file (found {} matches): {}",
                count,
                if edit.old_text.len() > 100 { format!("{}...", &edit.old_text[..100]) } else { edit.old_text.clone() }
            ));
        }
        result = result.replace(&edit.old_text, &edit.new_text);
    }
    Ok(result)
}

pub fn create_edit_tool(cwd: &str, options: Option<EditToolOptions>) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        name: "edit".to_string(),
        description: "Edit a file by performing targeted replacements. Each edit replaces exact text matches.".to_string(),
        label: "Edit".to_string(),
        parameters_schema: edit_parameters_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id: String, params: serde_json::Value, _signal: Option<tokio::sync::watch::Receiver<bool>>, _on_update: Option<Arc<dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync>>| {
            let cwd = cwd.clone();
            let operations = operations.clone();
            Box::pin(async move {
                let file_path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");

                let edits: Vec<ReplaceEdit> = match params.get("edits") {
                    Some(edits_val) => match serde_json::from_value::<Vec<ReplaceEdit>>(edits_val.clone()) {
                        Ok(e) => e,
                        Err(err) => {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!("Invalid edits: {}", err))],
                                details: serde_json::Value::Null,
                                terminate: None,
                            });
                        }
                    },
                    None => {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text("No edits provided")],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }
                };

                if edits.is_empty() {
                    return Ok(AgentToolResult {
                        content: vec![ContentBlock::text("No edits provided")],
                        details: serde_json::Value::Null,
                        terminate: None,
                    });
                }

                let absolute_path = path_utils::resolve_to_cwd(file_path, &cwd);
                let absolute_path_str = absolute_path.to_string_lossy().to_string();

                let content = match operations.read_file(&absolute_path_str).await {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!("Error reading file: {}", e))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }
                };

                match apply_edits(&content, &edits) {
                    Ok(new_content) => match operations.write_file(&absolute_path_str, &new_content).await {
                        Ok(()) => Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Successfully applied {} edit(s) to {}",
                                edits.len(), file_path
                            ))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        }),
                        Err(e) => Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!("Error writing file: {}", e))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        }),
                    },
                    Err(e) => Ok(AgentToolResult {
                        content: vec![ContentBlock::text(format!("Edit error: {}", e))],
                        details: serde_json::Value::Null,
                        terminate: None,
                    }),
                }
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_edits_single() {
        let content = "hello world";
        let edits = vec![ReplaceEdit { old_text: "world".to_string(), new_text: "rust".to_string() }];
        assert_eq!(apply_edits(content, &edits).unwrap(), "hello rust");
    }

    #[test]
    fn test_apply_edits_multiple() {
        let content = "foo bar baz";
        let edits = vec![
            ReplaceEdit { old_text: "foo".to_string(), new_text: "one".to_string() },
            ReplaceEdit { old_text: "baz".to_string(), new_text: "three".to_string() },
        ];
        assert_eq!(apply_edits(content, &edits).unwrap(), "one bar three");
    }

    #[test]
    fn test_apply_edits_not_found() {
        let content = "hello world";
        let edits = vec![ReplaceEdit { old_text: "notfound".to_string(), new_text: "replaced".to_string() }];
        assert!(apply_edits(content, &edits).is_err());
    }

    #[test]
    fn test_apply_edits_not_unique() {
        let content = "hello hello";
        let edits = vec![ReplaceEdit { old_text: "hello".to_string(), new_text: "hi".to_string() }];
        assert!(apply_edits(content, &edits).is_err());
    }
}