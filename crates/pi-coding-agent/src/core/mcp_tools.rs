//! Adapt MCP tools and results to pi's tool pipeline.
//!
//! Port of `packages/coding-agent/src/extensions/mcp/tools.ts`.

use std::io::Write;
use std::sync::Arc;

use base64::Engine as _;
use pi_mcp::{CallToolResult, ContentBlock, LlmContent, Tool as McpTool};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::core::mcp_runtime::McpServerConnection;
use crate::core::mcp_servers::{mcp_namespace, McpExposure};

/// Maximum model-facing text of an MCP result.
pub const MCP_OUTPUT_MAX_BYTES: usize = 20 * 1024;
/// Provider tool names are limited to 64 characters of `[A-Za-z0-9_-]`.
const MAX_TOOL_NAME_LENGTH: usize = 64;

/// `mcp__<server>__<tool>`, sanitized and shortened with a hash suffix.
pub fn create_mcp_tool_name(
    server: &str,
    tool: &str,
    is_taken: impl Fn(&str) -> bool,
) -> String {
    let name = format!("mcp__{server}__{tool}").replace(
        |character: char| !(character.is_ascii_alphanumeric() || character == '_'),
        "_",
    );
    if name.len() <= MAX_TOOL_NAME_LENGTH && !is_taken(&name) {
        return name;
    }
    let hash = Sha256::digest(format!("{server}\u{0}{tool}").as_bytes());
    let suffix = hash[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let keep = MAX_TOOL_NAME_LENGTH.saturating_sub(suffix.len() + 1);
    let mut truncated = name;
    truncated.truncate(keep.min(truncated.len()));
    format!("{truncated}_{suffix}")
}

/// Map an MCP exposure to pi's tool exposure.
pub fn to_tool_exposure(exposure: McpExposure) -> pi_extension_api::ToolExposure {
    match exposure {
        McpExposure::Codemode => pi_extension_api::ToolExposure::Deferred,
        McpExposure::Deferred => pi_extension_api::ToolExposure::Deferred,
        McpExposure::Direct => pi_extension_api::ToolExposure::Direct,
        McpExposure::Hidden => pi_extension_api::ToolExposure::Hidden,
    }
}

fn annotations(tool: &McpTool) -> Option<pi_extension_api::ToolAnnotations> {
    let annotations = tool.annotations.as_ref()?;
    let result = pi_extension_api::ToolAnnotations {
        title: annotations.title.clone(),
        read_only_hint: annotations.read_only_hint,
        destructive_hint: annotations.destructive_hint,
        idempotent_hint: annotations.idempotent_hint,
        open_world_hint: annotations.open_world_hint,
    };
    let has_hint = result.read_only_hint.is_some()
        || result.destructive_hint.is_some()
        || result.idempotent_hint.is_some()
        || result.open_world_hint.is_some();
    if has_hint {
        Some(result)
    } else {
        None
    }
}

fn parameters(tool: &McpTool) -> Value {
    let mut schema = tool.input_schema.clone();
    if !schema.contains_key("type") {
        schema.insert("type".to_string(), Value::String("object".to_string()));
    }
    if !schema.contains_key("properties") {
        schema.insert("properties".to_string(), Value::Object(Map::new()));
    }
    Value::Object(schema)
}

fn output_schema(tool: &McpTool) -> Value {
    json!({
        "type": "object",
        "properties": {
            "content": {"type": "array", "items": {"type": "object", "properties": {
                "type": {"enum": ["text", "image", "audio", "resource_link", "resource"]},
                "text": {"type": "string"},
                "data": {"type": "string"},
                "mimeType": {"type": "string"}
            }, "required": ["type"], "additionalProperties": true}},
            "structuredContent": tool.output_schema.clone().map(Value::Object).unwrap_or_else(|| json!({})),
            "isError": {"type": "boolean"}
        },
        "required": ["content"],
        "additionalProperties": true
    })
}

fn llm_content_to_json(content: LlmContent) -> Value {
    match content {
        LlmContent::Text { text } => {
            serde_json::to_value(pi_agent_core::pi_ai_types::text_block(text))
                .unwrap_or(Value::Null)
        }
        LlmContent::Image { data, mime_type } => {
            serde_json::to_value(pi_agent_core::pi_ai_types::image_block(data, mime_type))
                .unwrap_or(Value::Null)
        }
    }
}

fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .map(|block| block.get("text").and_then(Value::as_str).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_middle(text: &str, max_bytes: usize) -> Option<(String, usize, usize)> {
    if text.len() <= max_bytes {
        return None;
    }
    let half = max_bytes / 2;
    let mut start_end = half;
    while start_end > 0 && !text.is_char_boundary(start_end) {
        start_end -= 1;
    }
    let mut end_start = text.len() - half;
    while end_start < text.len() && !text.is_char_boundary(end_start) {
        end_start += 1;
    }
    // v1.5.0 `truncateMiddle`: `…N chars truncated…` with the scalar count of
    // the removed middle, and `split("\n")` line counting.
    let removed_chars = text[start_end..end_start].chars().count();
    let total_lines = text.split('\n').count() - usize::from(text.ends_with('\n'));
    Some((
        format!(
            "{}…{removed_chars} chars truncated…{}",
            &text[..start_end],
            &text[end_start..]
        ),
        text.len(),
        total_lines,
    ))
}

fn save_full_output(text: &str) -> Result<String, String> {
    let path = std::env::temp_dir().join(format!("pi-mcp-{}.txt", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| format!("could not save full output: {error}"))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("could not save full output: {error}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Limit model-facing text; returns the details path when truncated.
pub fn limit_content(content: Vec<Value>) -> (Vec<Value>, Option<String>) {
    let text = text_of(&content);
    let Some((truncated, total_bytes, total_lines)) = truncate_middle(&text, MCP_OUTPUT_MAX_BYTES)
    else {
        return (content, None);
    };
    let tokens = total_bytes.div_ceil(4);
    let (full_output, where_hint) = match save_full_output(&text) {
        Ok(path) => {
            let hint = format!("[Full output: {path} (read it with offset/limit)]");
            (Some(path), hint)
        }
        Err(error) => (None, format!("[Could not save the full output: {error}]")),
    };
    let mut limited = vec![json!({
        "type": "text",
        "text": format!(
            "Warning: truncated output (original token count: {tokens})\nTotal output lines: {total_lines}\n\n{truncated}\n\n{where_hint}"
        )
    })];
    limited.extend(
        content
            .into_iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("image")),
    );
    (limited, full_output)
}

/// Whether a MIME type is text-like (v1.5.0 `isTextMimeType`).
fn is_text_mime(mime: Option<&str>) -> bool {
    let Some(mime) = mime else {
        return false;
    };
    let base = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    base.starts_with("text/")
        || matches!(
            base.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-yaml"
                | "application/yaml"
                | "application/toml"
        )
}

/// Extension (including the dot) from a resource URI, when present.
fn extension_of(uri: &str) -> String {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default()
}

/// Save a binary resource to an owner-only temp file.
fn save_binary_output(data: &[u8], extension: &str) -> Result<String, String> {
    let path = std::env::temp_dir().join(format!(
        "pi-mcp-{}{extension}",
        uuid::Uuid::new_v4()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| format!("could not save binary resource: {error}"))?;
    file.write_all(data)
        .map_err(|error| format!("could not save binary resource: {error}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Convert one MCP content block (v1.5.0 `blockToContent`): resource links get
/// a readable description, text resources become text, image resources become
/// images, and other binary resources are decoded or saved to a temp file.
fn block_to_content(server: &str, block: &ContentBlock) -> Vec<Value> {
    match block {
        ContentBlock::ResourceLink {
            uri,
            name,
            title,
            description,
            mime_type,
            size,
            ..
        } => {
            let mut details = Vec::new();
            if let Some(mime) = mime_type {
                details.push(mime.clone());
            }
            if let Some(size) = size {
                details.push(crate::core::tools::truncate::format_size(*size as usize));
            }
            let details = if details.is_empty() {
                String::new()
            } else {
                format!(" ({})", details.join(", "))
            };
            let description = description
                .as_ref()
                .map(|description| format!(": {description}"))
                .unwrap_or_default();
            let read = format!(
                ". Read it with {} (server \"{server}\")",
                crate::core::mcp_servers::READ_MCP_RESOURCE_TOOL
            );
            vec![json!({
                "type": "text",
                "text": format!(
                    "[Resource {uri} \"{}\"{details}{description}{read}]",
                    title.as_deref().unwrap_or(name)
                )
            })]
        }
        ContentBlock::Resource { resource, .. } => {
            if let Some(text) = resource.text() {
                return vec![json!({"type": "text", "text": text})];
            }
            let mime = resource.mime_type();
            if mime.is_some_and(|mime| mime.starts_with("image/")) {
                return pi_mcp::to_llm_content(&CallToolResult {
                    content: vec![block.clone()],
                    ..Default::default()
                })
                .into_iter()
                .map(llm_content_to_json)
                .collect();
            }
            let blob = resource.blob().unwrap_or_default();
            let data = base64::engine::general_purpose::STANDARD
                .decode(blob)
                .unwrap_or_default();
            if is_text_mime(mime) {
                return vec![json!({
                    "type": "text",
                    "text": String::from_utf8_lossy(&data)
                })];
            }
            let kind = format!(
                "{}, {}",
                mime.unwrap_or("unknown type"),
                crate::core::tools::truncate::format_size(data.len())
            );
            let uri = resource.uri();
            match save_binary_output(&data, &extension_of(uri)) {
                Ok(path) => vec![json!({
                    "type": "text",
                    "text": format!("[Binary resource {uri} ({kind}) saved to {path}]")
                })],
                Err(error) => vec![json!({
                    "type": "text",
                    "text": format!("[Binary resource {uri} ({kind}) could not be saved: {error}]")
                })],
            }
        }
        _ => pi_mcp::to_llm_content(&CallToolResult {
            content: vec![block.clone()],
            ..Default::default()
        })
        .into_iter()
        .map(llm_content_to_json)
        .collect(),
    }
}

/// Convert an MCP `CallToolResult` into a pi `ToolCallOutput`.
pub fn convert_mcp_result(server: &str, tool: &str, result: CallToolResult) -> pi_extension_api::ToolCallOutput {
    let mut content: Vec<Value> = result
        .content
        .iter()
        .flat_map(|block| block_to_content(server, block))
        .collect();
    if content.is_empty() {
        if let Some(structured) = &result.structured_content {
            content.push(json!({
                "type": "text",
                "text": serde_json::to_string_pretty(structured)
                    .unwrap_or_else(|_| "null".to_string())
            }));
        }
    }
    if result.is_error && text_of(&content).is_empty() {
        content.push(llm_content_to_json(LlmContent::Text {
            text: format!("MCP tool {server}/{tool} returned an error"),
        }));
    }
    let (content, full_output_path) = limit_content(content);
    let mut details = json!({
        "server": server,
        "tool": tool,
        "isError": result.is_error,
    });
    if let Some(structured) = &result.structured_content {
        details["structuredContent"] = structured.clone();
    }
    if let Some(path) = &full_output_path {
        details["fullOutputPath"] = Value::String(path.clone());
    }
    pi_extension_api::ToolCallOutput {
        content,
        details: Some(details),
        is_error: result.is_error,
        terminate: None,
    }
}

/// Build a pi tool definition for one MCP tool.
pub fn create_mcp_tool_definition(
    connection: &McpServerConnection,
    tool: &McpTool,
    name: String,
    exposure: McpExposure,
) -> pi_extension_api::ToolDefinition {
    let server = connection.name().to_string();
    let tool_name = tool.name.clone();
    let description = tool
        .description
        .clone()
        .or_else(|| tool.title.clone())
        .unwrap_or_else(|| format!("MCP tool {tool_name} from server {server}"));
    let namespace = pi_extension_api::ToolNamespace {
        name: mcp_namespace(&server),
        aliases: vec![server.clone()],
        description: connection.entry().config.description.clone(),
        instructions: connection.instructions(),
    };
    let connection = connection.clone();
    let label = format!("{server}/{}", tool.name);
    let execute: pi_extension_api::ToolExecuteFn = Arc::new(
        move |_tool_call_id: String, params: Value, signal: Option<tokio::sync::watch::Receiver<bool>>| {
            let connection = connection.clone();
            let server = server.clone();
            let tool_name = tool_name.clone();
            Box::pin(async move {
                let arguments = params.as_object().cloned().unwrap_or_default();
                let cancel = tokio_util::sync::CancellationToken::new();
                if let Some(signal) = signal {
                    let cancel = cancel.clone();
                    tokio::spawn(async move {
                        pi_extension_api::wait_aborted(&mut Some(signal)).await;
                        cancel.cancel();
                    });
                }
                let result = connection
                    .call_tool(&tool_name, arguments, Some(cancel))
                    .await
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
                Ok(convert_mcp_result(&server, &tool_name, result))
            })
        },
    );
    pi_extension_api::ToolDefinition {
        name,
        label: Some(label),
        description,
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: Some(parameters(tool)),
        output_schema: Some(output_schema(tool)),
        exposure: to_tool_exposure(exposure),
        namespace: Some(namespace),
        annotations: annotations(tool),
        execute: Some(execute),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn sanitizes_and_hashes_long_names() {
        let name = create_mcp_tool_name("my-server", "read file", |_| false);
        assert_eq!(name, "mcp__my-server__read_file".replace('-', "_"));
        let long = "x".repeat(80);
        let name = create_mcp_tool_name("server", &long, |_| false);
        assert!(name.len() <= MAX_TOOL_NAME_LENGTH);
        assert!(name.contains('_'));
    }

    #[test]
    fn truncates_large_text_and_keeps_images() {
        let text = "a".repeat(MCP_OUTPUT_MAX_BYTES + 100);
        let content = vec![
            json!({"type": "text", "text": text}),
            json!({"type": "image", "data": "AA==", "mimeType": "image/png"}),
        ];
        let (limited, path) = limit_content(content);
        assert!(path.is_some());
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[1]["type"], "image");
        assert!(limited[0]["text"].as_str().unwrap().contains("truncated"));
    }

    #[test]
    fn structured_content_without_blocks_becomes_text() {
        let result = CallToolResult {
            content: Vec::new(),
            structured_content: Some(serde_json::json!({"answer": 42})),
            is_error: false,
            meta: None,
        };
        let output = convert_mcp_result("s", "t", result);
        assert_eq!(output.content.len(), 1);
        assert!(text_of(&output.content).contains("\"answer\": 42"));
        assert_eq!(
            output.details.as_ref().unwrap()["structuredContent"]["answer"],
            42
        );
    }

    #[test]
    fn converts_error_result() {
        let result = CallToolResult {
            content: Vec::new(),
            structured_content: None,
            is_error: true,
            meta: None,
        };
        let output = convert_mcp_result("s", "t", result);
        assert!(output.is_error);
        assert!(text_of(&output.content).contains("returned an error"));
    }
}
