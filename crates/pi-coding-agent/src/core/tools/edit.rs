use std::fmt;
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::edit_diff::{
    self, apply_edits_to_normalized_content, compute_edits_diff, detect_line_ending,
    generate_diff_string, generate_unified_patch, normalize_to_lf, restore_line_endings, strip_bom,
    Edit, EditDiffResult,
};
use super::file_mutation_queue::with_file_mutation_queue;
use super::path_utils;

// ============================================================================
// Types
// ============================================================================

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
pub struct EditToolDetails {
    /// Display-oriented diff of the changes made.
    pub diff: Option<String>,
    /// Standard unified patch of the changes made.
    pub patch: Option<String>,
    /// Line number of the first change in the new file (for editor navigation).
    pub first_changed_line: Option<usize>,
}

// ============================================================================
// EditOperations trait
// ============================================================================

/// Pluggable operations for the edit tool.
/// Override these to delegate file editing to remote systems (for example SSH).
pub trait EditOperations: Send + Sync {
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

    fn write_file(
        &self,
        path: &str,
        content: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    >;

    /// Check if file is readable and writable (throw if not).
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
// LocalEditOperations
// ============================================================================

pub struct LocalEditOperations;

impl EditOperations for LocalEditOperations {
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

    fn write_file(
        &self,
        path: &str,
        content: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
                + Send,
        >,
    > {
        let path = path.to_string();
        let content = content.to_string();
        Box::pin(async move {
            tokio::fs::write(&path, &content)
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
            // Check if file exists and is readable
            tokio::fs::metadata(&path)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(())
        })
    }
}

// ============================================================================
// EditToolOptions
// ============================================================================

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

// ============================================================================
// Parameters schema
// ============================================================================

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
                            "description": "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."
                        },
                        "newText": {
                            "type": "string",
                            "description": "Replacement text for this targeted edit."
                        }
                    },
                    "required": ["oldText", "newText"],
                    "additionalProperties": false
                },
                "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."
            }
        },
        "required": ["path", "edits"],
        "additionalProperties": false
    })
}

// ============================================================================
// prepare_arguments
// ============================================================================

/// Prepare edit arguments, handling legacy format (oldText/newText at top level)
/// and JSON-stringified edits array.
fn prepare_edit_arguments(params: &serde_json::Value) -> serde_json::Value {
    if !params.is_object() {
        return params.clone();
    }

    let mut args = params.as_object().unwrap().clone();

    // Some models send edits as a JSON string instead of an array
    if let Some(edits_val) = args.get("edits") {
        if let Some(edits_str) = edits_val.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(edits_str) {
                if parsed.is_array() {
                    args.insert("edits".to_string(), parsed);
                }
            }
        }
    }

    // Handle legacy format: oldText/newText at top level
    let has_legacy_old = args.contains_key("oldText") && args.get("oldText").and_then(|v| v.as_str()).is_some();
    let has_legacy_new = args.contains_key("newText") && args.get("newText").and_then(|v| v.as_str()).is_some();

    if has_legacy_old && has_legacy_new {
        let old_text = args.get("oldText").unwrap().as_str().unwrap().to_string();
        let new_text = args.get("newText").unwrap().as_str().unwrap().to_string();

        let mut edits = match args.get("edits") {
            Some(edits_val) if edits_val.is_array() => edits_val.as_array().unwrap().clone(),
            _ => Vec::new(),
        };

        edits.push(serde_json::json!({
            "oldText": old_text,
            "newText": new_text,
        }));

        args.insert("edits".to_string(), serde_json::Value::Array(edits));
        args.remove("oldText");
        args.remove("newText");
    }

    serde_json::Value::Object(args)
}

// ============================================================================
// create_edit_tool
// ============================================================================

pub fn create_edit_tool(
    cwd: &str,
    options: Option<EditToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        name: "edit".to_string(),
        description: "Edit a file by performing targeted replacements. Each edit replaces exact text matches with fuzzy fallback.".to_string(),
        label: "Edit".to_string(),
        parameters_schema: edit_parameters_schema(),
        execution_mode: None,
        prepare_arguments: Some(Arc::new(prepare_edit_arguments)),
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

                    // Use file mutation queue to serialize edits to the same file
                    let queue_path = absolute_path_str.clone();
                    let abs_path = absolute_path_str.clone();
                    let fp = file_path.to_string();
                    let ops = operations.clone();
                    let sig = signal.clone();
                    let ed = edits.clone();

                    let result = with_file_mutation_queue(&queue_path, move || {
                        let abs_path = abs_path.clone();
                        let fp = fp.clone();
                        let ops = ops.clone();
                        let sig = sig.clone();
                        let ed = ed.clone();
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

                            // Check if file exists and is accessible
                            ops.access(&abs_path).await.map_err(|e| {
                                let err_msg = if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                                    format!("Error code: {}", io_err.kind())
                                } else {
                                    e.to_string()
                                };
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    format!("Could not edit file: {}. {}.", fp, err_msg),
                                )) as Box<dyn std::error::Error + Send + Sync>
                            })?;

                            throw_if_aborted()?;

                            // Read the file
                            let buffer = ops.read_file(&abs_path).await?;
                            let raw_content = String::from_utf8_lossy(&buffer).to_string();
                            throw_if_aborted()?;

                            // Strip BOM before matching
                            let bom_result = strip_bom(&raw_content);
                            let content = &bom_result.text;
                            let original_ending = detect_line_ending(content);
                            let normalized_content = normalize_to_lf(content);

                            // Convert edits to edit-diff format
                            let diff_edits: Vec<Edit> = ed
                                .iter()
                                .map(|e| Edit {
                                    old_text: e.old_text.clone(),
                                    new_text: e.new_text.clone(),
                                })
                                .collect();

                            // Apply edits using edit-diff's fuzzy matching
                            let applied = apply_edits_to_normalized_content(
                                &normalized_content,
                                &diff_edits,
                                &fp,
                            )
                            .map_err(|e| {
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    e.to_string(),
                                )) as Box<dyn std::error::Error + Send + Sync>
                            })?;

                            throw_if_aborted()?;

                            // Restore line endings and BOM
                            let final_content = bom_result.bom.clone()
                                + &restore_line_endings(&applied.new_content, original_ending);

                            // Write the file
                            ops.write_file(&abs_path, &final_content).await?;
                            throw_if_aborted()?;

                            // Generate diff and patch for details
                            let diff_result = generate_diff_string(&applied.base_content, &applied.new_content, 4);
                            let patch = generate_unified_patch(&fp, &applied.base_content, &applied.new_content, 4);

                            Ok::<AgentToolResult<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>>(
                                AgentToolResult {
                                    content: vec![ContentBlock::text(format!(
                                        "Successfully replaced {} block(s) in {}.",
                                        ed.len(),
                                        fp
                                    ))],
                                    details: serde_json::to_value(EditToolDetails {
                                        diff: Some(diff_result.diff),
                                        patch: Some(patch),
                                        first_changed_line: diff_result.first_changed_line,
                                    }).unwrap_or(serde_json::Value::Null),
                                    terminate: None,
                                }
                            )
                        }
                    }).await;

                    match result {
                        Ok(r) => Ok(r),
                        Err(e) => Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!("Edit error: {}", e))],
                            details: serde_json::to_value(EditToolDetails::default())
                                .unwrap_or(serde_json::Value::Null),
                            terminate: None,
                        }),
                    }
                })
            },
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prepare_arguments_legacy_format() {
        let params = serde_json::json!({
            "path": "test.txt",
            "oldText": "hello",
            "newText": "world"
        });
        let result = prepare_edit_arguments(&params);
        assert!(result.get("edits").is_some());
        assert!(result.get("oldText").is_none());
        assert!(result.get("newText").is_none());
        let edits = result.get("edits").unwrap().as_array().unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["oldText"], "hello");
        assert_eq!(edits[0]["newText"], "world");
    }

    #[test]
    fn test_prepare_arguments_legacy_with_existing_edits() {
        let params = serde_json::json!({
            "path": "test.txt",
            "oldText": "hello",
            "newText": "world",
            "edits": [
                {"oldText": "foo", "newText": "bar"}
            ]
        });
        let result = prepare_edit_arguments(&params);
        let edits = result.get("edits").unwrap().as_array().unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0]["oldText"], "foo");
        assert_eq!(edits[1]["oldText"], "hello");
    }

    #[test]
    fn test_prepare_arguments_json_string_edits() {
        let params = serde_json::json!({
            "path": "test.txt",
            "edits": "[{\"oldText\":\"hello\",\"newText\":\"world\"}]"
        });
        let result = prepare_edit_arguments(&params);
        let edits = result.get("edits").unwrap().as_array().unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["oldText"], "hello");
    }

    #[test]
    fn test_prepare_arguments_normal_format() {
        let params = serde_json::json!({
            "path": "test.txt",
            "edits": [
                {"oldText": "hello", "newText": "world"}
            ]
        });
        let result = prepare_edit_arguments(&params);
        let edits = result.get("edits").unwrap().as_array().unwrap();
        assert_eq!(edits.len(), 1);
    }

    #[test]
    fn test_prepare_arguments_no_edits() {
        let params = serde_json::json!({
            "path": "test.txt"
        });
        let result = prepare_edit_arguments(&params);
        assert!(result.get("edits").is_none());
    }
}
