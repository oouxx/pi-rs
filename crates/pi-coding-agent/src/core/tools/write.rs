#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unnecessary_struct_initialization,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_closure,
    clippy::missing_const_for_fn,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::manual_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::redundant_clone,
    clippy::clone_on_ref_ptr,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::match_same_arms,
    clippy::needless_continue,
    clippy::items_after_statements,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::derive_partial_eq_without_eq,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_lit_as_bytes,
    clippy::trivially_copy_pass_by_ref,
    clippy::use_self,
    clippy::significant_drop_tightening,
    clippy::default_trait_access,
    clippy::iter_with_drain,
    clippy::if_not_else,
    clippy::explicit_iter_loop,
    clippy::assigning_clones,
    clippy::implicit_hasher,
    clippy::ignored_unit_patterns,
    clippy::missing_fields_in_debug,
    clippy::or_fun_call,
    clippy::too_long_first_doc_paragraph,
    clippy::manual_string_new,
    clippy::single_match_else,
    clippy::significant_drop_in_scrutinee,
    clippy::needless_collect,
    clippy::duplicated_attributes,
    clippy::similar_names,
    clippy::needless_raw_string_hashes,
    clippy::unnested_or_patterns,
)]
use std::fmt;
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::file_mutation_queue::with_file_mutation_queue;
use super::path_utils;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteToolInput {
    pub path: String,
    pub content: String,
}

// ============================================================================
// WriteOperations trait
// ============================================================================

/// Pluggable operations for the write tool.
/// Override these to delegate file writing to remote systems (for example SSH).
pub trait WriteOperations: Send + Sync {
    fn write_file(
        &self,
        path: &str,
        content: &str,
    ) -> crate::core::tools::AsyncOpResult<()>;

    fn mkdir(
        &self,
        dir: &str,
    ) -> crate::core::tools::AsyncOpResult<()>;
}

// ============================================================================
// LocalWriteOperations
// ============================================================================

pub struct LocalWriteOperations;

impl WriteOperations for LocalWriteOperations {
    fn write_file(
        &self,
        path: &str,
        content: &str,
    ) -> crate::core::tools::AsyncOpResult<()> {
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
    ) -> crate::core::tools::AsyncOpResult<()> {
        let dir = dir.to_string();
        Box::pin(async move {
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
    }
}

// ============================================================================
// WriteToolOptions
// ============================================================================

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

// ============================================================================
// Parameters schema
// ============================================================================

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

// ============================================================================
// create_write_tool
// ============================================================================

pub fn create_write_tool(
    cwd: &str,
    options: Option<WriteToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        name: "write".to_string(),
        description: "Write content to a file. Creates the file and any parent directories if they don't exist."
            .to_string(),
        label: "Write".to_string(),
        parameters_schema: write_parameters_schema(),
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
                    let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");

                    let absolute_path = path_utils::resolve_to_cwd(file_path, &cwd);
                    let absolute_path_str = absolute_path.to_string_lossy().to_string();

                    let result = with_file_mutation_queue(&absolute_path_str, || {
                        let abs_path = absolute_path_str.clone();
                        let fp = file_path.to_string();
                        let ops = operations.clone();
                        let sig = signal.clone();
                        let cnt = content.to_string();
                        async move {
                            let throw_if_aborted = || {
                                if let Some(ref rx) = sig {
                                    if *rx.borrow() {
                                        return Err(Box::new(std::io::Error::new(
                                            std::io::ErrorKind::Interrupted,
                                            "Operation aborted",
                                        )) as Box<dyn std::error::Error + Send + Sync>);
                                    }
                                }
                                Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
                            };

                            throw_if_aborted()?;

                            // Create parent directories if needed
                            if let Some(parent) = std::path::Path::new(&abs_path).parent() {
                                let parent_str = parent.to_string_lossy().to_string();
                                if !parent.exists() {
                                    ops.mkdir(&parent_str).await.map_err(|e| {
                                        Box::new(std::io::Error::other(
format!("Error creating directory: {}", e),
    )) as Box<dyn std::error::Error + Send + Sync>
                                    })?;
                                }
                            }

                            throw_if_aborted()?;

                            // Write the file contents
                            let byte_count = cnt.len();
                            ops.write_file(&abs_path, &cnt).await.map_err(|e| {
                                Box::new(std::io::Error::other(
format!("Error writing file: {}", e),
    )) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                            throw_if_aborted()?;

                            Ok::<AgentToolResult<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>>(
                                AgentToolResult {
                                    content: vec![ContentBlock::text(format!(
                                        "Successfully wrote {} bytes to {}",
                                        byte_count, fp
                                    ))],
                                    details: serde_json::Value::Null,
                                    usage: None,
                                    added_tool_names: None,
                                    terminate: None,
                                }
                            )
                        }
                    }).await;

                    match result {
                        Ok(r) => Ok(r),
                        Err(e) => Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!("Write error: {}", e))],
                            details: serde_json::Value::Null,
                            usage: None,
                            added_tool_names: None,
                            terminate: None,
                        }),
                    }
                })
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_write_tool_input_deserialization() {
        let json = serde_json::json!({
            "path": "/tmp/test.txt",
            "content": "hello world"
        });
        let input: WriteToolInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.path, "/tmp/test.txt");
        assert_eq!(input.content, "hello world");
    }

    #[test]
    fn test_write_parameters_schema() {
        let schema = write_parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["content"].is_object());
        assert_eq!(schema["required"][0], "path");
        assert_eq!(schema["required"][1], "content");
    }

    #[test]
    fn test_write_tool_creation() {
        let tool = create_write_tool("/tmp", None);
        assert_eq!(tool.name, "write");
        assert!(tool.description.contains("Write"));
        assert!(tool.parameters_schema.is_object());
    }
}
