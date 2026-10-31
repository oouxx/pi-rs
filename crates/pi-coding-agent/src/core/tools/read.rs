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
    fn read_file(&self, path: &str) -> crate::core::tools::AsyncOpResult<Vec<u8>>;

    /// Check if file is readable (throw if not).
    fn access(&self, path: &str) -> crate::core::tools::AsyncOpResult<()>;
}

// ============================================================================
// LocalReadOperations
// ============================================================================

pub struct LocalReadOperations;

impl ReadOperations for LocalReadOperations {
    fn read_file(&self, path: &str) -> crate::core::tools::AsyncOpResult<Vec<u8>> {
        let path = path.to_string();
        Box::pin(async move {
            tokio::fs::read(&path)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
    }

    fn access(&self, path: &str) -> crate::core::tools::AsyncOpResult<()> {
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

/// Async provider for the model currently bound to the session, so the image
/// branch can consult `model.input` (non-vision note) and
/// `model.inputLimits.images.resize` at execution time (TS `ctx?.model`).
pub type ReadModelProvider = Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<pi_ai::types::Model>> + Send>,
        > + Send
        + Sync,
>;

#[derive(Clone)]
pub struct ReadToolOptions {
    pub operations: Arc<dyn ReadOperations>,
    /// Auto-resize images to inline provider limits (TS `autoResizeImages`,
    /// default true).
    pub auto_resize_images: bool,
    /// Live model lookup (TS `ctx?.model`). `None` = no model metadata, which
    /// means the built-in resize defaults and no non-vision note.
    pub model_provider: Option<ReadModelProvider>,
    /// Fallback resize profile when the execution context has no model
    /// metadata (TS `options.resizeOptions`).
    pub resize_options: Option<pi_ai::types::ModelImageResizeOptions>,
}

impl fmt::Debug for ReadToolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadToolOptions")
            .field("auto_resize_images", &self.auto_resize_images)
            .field("has_model_provider", &self.model_provider.is_some())
            .field("resize_options", &self.resize_options)
            .finish()
    }
}

impl Default for ReadToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalReadOperations),
            auto_resize_images: true,
            model_provider: None,
            resize_options: None,
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

/// Polled abort check (TS reads `signal.aborted` at each step).
fn aborted(signal: &Option<tokio::sync::watch::Receiver<bool>>) -> bool {
    signal.as_ref().map(|rx| *rx.borrow()).unwrap_or(false)
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
    let auto_resize_images = opts.auto_resize_images;
    let model_provider = opts.model_provider.clone();
    let fallback_resize_options = opts.resize_options.clone();

    AgentTool {
        constrained_sampling: super::preferred_json_schema_sampling(),
        name: "read".to_string(),
        description: format!(
            "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). \
             Images are sent as attachments. For text files, output is truncated to {} lines or {}KB \
             (whichever is hit first). Use offset/limit for large files. When you need the full file, \
             continue with offset until complete.",
            truncate::DEFAULT_MAX_LINES,
            DEFAULT_MAX_BYTES / 1024
        ),
        label: "read".to_string(),
        prompt_snippet: Some("Read file contents".to_string()),
        prompt_guidelines: Some(vec!["Use read to examine files instead of cat or sed.".to_string()]),
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
                let auto_resize_images = auto_resize_images;
                let model_provider = model_provider.clone();
                let fallback_resize_options = fallback_resize_options.clone();
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

                    // Check if file exists and is readable. TS rejects with the
                    // raw fs error (`ops.access` throws), which the agent loop
                    // turns into an error tool result.
                    operations.access(&absolute_path_str).await?;

                    if aborted(&signal) {
                        return Err("Operation aborted".to_string().into());
                    }

                    let bytes = operations.read_file(&absolute_path_str).await?;

                    if aborted(&signal) {
                        return Err("Operation aborted".to_string().into());
                    }

                    // Images are returned as inline attachments (text note + optional
                    // resized image). Detected from magic bytes before the UTF-8
                    // decode that would report "Binary file".
                    if let Some(mime) = crate::utils::mime::detect_supported_image_mime_type(&bytes) {
                        let mime = mime.to_string();
                        // TS `ctx?.model`: the live model decides the resize
                        // profile and whether images are supported at all.
                        let model = match &model_provider {
                            Some(provider) => provider().await,
                            None => None,
                        };
                        let non_vision_note = model.as_ref().and_then(|m| {
                            if m.input.iter().any(|i| i == "image") {
                                None
                            } else {
                                Some(
                                    "[Current model does not support images. The image will be omitted from this request.]",
                                )
                            }
                        });
                        let resize = model
                            .as_ref()
                            .and_then(|m| m.input_limits.as_ref())
                            .and_then(|limits| limits.images.as_ref())
                            .and_then(|images| images.resize.clone())
                            .or(fallback_resize_options);
                        let content = match crate::utils::image_process::process_image(
                            &bytes,
                            &mime,
                            auto_resize_images,
                            resize.as_ref(),
                        ) {
                            Ok(processed) => {
                                let mut note = format!("Read image file [{}]", processed.mime_type);
                                for hint in &processed.hints {
                                    note.push('\n');
                                    note.push_str(hint);
                                }
                                if let Some(note_extra) = non_vision_note {
                                    note.push('\n');
                                    note.push_str(note_extra);
                                }
                                vec![
                                    ContentBlock::text(note),
                                    ContentBlock::Image {
                                        data: processed.data,
                                        mime_type: processed.mime_type,
                                    },
                                ]
                            }
                            Err(message) => {
                                let mut note = format!("Read image file [{mime}]\n{message}");
                                if let Some(note_extra) = non_vision_note {
                                    note.push('\n');
                                    note.push_str(note_extra);
                                }
                                vec![ContentBlock::text(note)]
                            }
                        };
                        return Ok(AgentToolResult {
                            content,
                            details: serde_json::to_value(ReadToolDetails::default())
                                .unwrap_or(serde_json::Value::Null),
                            usage: None,
                            terminate: None,
                        });
                    }

                    // TS reads text via `buffer.toString("utf-8")`, i.e. invalid
                    // sequences become U+FFFD instead of a "Binary file" note —
                    // binary files are decoded lossily and then truncated like
                    // any other text.
                    let text_content = String::from_utf8_lossy(&bytes).into_owned();

                    let all_lines: Vec<&str> = text_content.split('\n').collect();
                    let total_file_lines = all_lines.len();
                    let start_line = offset.map(|o| if o > 0 { o - 1 } else { 0 }).unwrap_or(0);

                    if start_line >= all_lines.len() {
                        // TS `reject`s here (error tool result).
                        return Err(format!(
                            "Offset {} is beyond end of file ({} lines total)",
                            offset.unwrap_or(0),
                            total_file_lines
                        )
                        .into());
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

                    if aborted(&signal) {
                        return Err("Operation aborted".to_string().into());
                    }

                    Ok(AgentToolResult {
                        content: vec![ContentBlock::text(output_text)],
                        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                        usage: None,
                        terminate: None,
                    })
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
    fn test_read_tool_input_deserialization() {
        let json = serde_json::json!({
            "path": "/tmp/test.txt",
            "offset": 10,
            "limit": 20
        });
        let input: ReadToolInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.path, "/tmp/test.txt");
        assert_eq!(input.offset, Some(10));
        assert_eq!(input.limit, Some(20));
    }

    #[test]
    fn test_read_tool_input_minimal() {
        let json = serde_json::json!({
            "path": "/tmp/test.txt"
        });
        let input: ReadToolInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.path, "/tmp/test.txt");
        assert_eq!(input.offset, None);
        assert_eq!(input.limit, None);
    }

    #[test]
    fn test_read_parameters_schema() {
        let schema = read_parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["offset"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        assert_eq!(schema["required"][0], "path");
    }

    #[test]
    fn test_read_tool_creation() {
        let tool = create_read_tool("/tmp", None);
        assert_eq!(tool.name, "read");
        assert!(tool.description.contains("Read the contents"));
        assert!(tool.parameters_schema.is_object());
    }

    /// Image files are returned as a text note + inline image block (TS read
    /// tool image branch), detected from magic bytes rather than the UTF-8
    /// "Binary file" fallback.
    #[tokio::test]
    async fn read_tool_returns_image_content_block() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pic.png");
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
            .save(&file)
            .unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let tool = create_read_tool(&cwd, None);
        let result = (tool.execute)(
            "tool-1".to_string(),
            serde_json::json!({ "path": "pic.png" }),
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.content.len(), 2, "text note + image block");
        match &result.content[0] {
            ContentBlock::Text { text, .. } => {
                assert!(text.contains("Read image file [image/png]"), "got: {text}")
            }
            other => panic!("expected text note, got {other:?}"),
        }
        match &result.content[1] {
            ContentBlock::Image { data, mime_type } => {
                assert_eq!(mime_type, "image/png");
                assert!(!data.is_empty());
            }
            other => panic!("expected image block, got {other:?}"),
        }
    }

    /// Non-UTF-8 files are decoded lossily like TS `buffer.toString("utf-8")`
    /// (U+FFFD for invalid bytes) and then go through normal text truncation,
    /// instead of the old "Binary file" note.
    #[tokio::test]
    async fn read_tool_decodes_binary_lossily() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("blob.bin");
        std::fs::write(&file, [0xff, 0xfe, b'h', b'i', 0x00, b'\n']).unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let tool = create_read_tool(&cwd, None);
        let result = (tool.execute)(
            "tool-1".to_string(),
            serde_json::json!({ "path": "blob.bin" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = match &result.content[0] {
            ContentBlock::Text { text, .. } => text.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(!text.contains("Binary file"), "got: {text}");
        assert!(
            text.contains('\u{FFFD}'),
            "invalid bytes become U+FFFD: {text:?}"
        );
        assert!(text.contains("hi"), "valid bytes are preserved: {text:?}");
    }
}
