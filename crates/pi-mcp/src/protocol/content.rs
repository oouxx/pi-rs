//! MCP content blocks and the model-facing conversion.
//!
//! Port of `packages/mcp/src/protocol/content.ts`. `LlmContent` deliberately
//! mirrors the TS shape so `pi-coding-agent` can translate it to `pi-ai`
//! blocks without `pi-mcp` depending on `pi-ai`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Content annotations attached to blocks and resources.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentAnnotations {
    /// Intended audience.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<String>>,
    /// Importance hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    /// Last modification timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

/// Text contents of a resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextResourceContents {
    /// Resource URI.
    pub uri: String,
    /// Optional MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Text body.
    pub text: String,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

/// Base64 blob contents of a resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobResourceContents {
    /// Resource URI.
    pub uri: String,
    /// Optional MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Base64-encoded body.
    pub blob: String,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

/// Either kind of resource contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceContents {
    /// Text resource.
    Text(TextResourceContents),
    /// Binary resource.
    Blob(BlobResourceContents),
}

impl ResourceContents {
    /// Resource URI.
    pub fn uri(&self) -> &str {
        match self {
            Self::Text(text) => &text.uri,
            Self::Blob(blob) => &blob.uri,
        }
    }

    /// MIME type, when present.
    pub fn mime_type(&self) -> Option<&str> {
        match self {
            Self::Text(text) => text.mime_type.as_deref(),
            Self::Blob(blob) => blob.mime_type.as_deref(),
        }
    }

    /// Text body, when this is a text resource.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(&text.text),
            Self::Blob(_) => None,
        }
    }

    /// Base64 body, when this is a binary resource.
    pub fn blob(&self) -> Option<&str> {
        match self {
            Self::Text(_) => None,
            Self::Blob(blob) => Some(&blob.blob),
        }
    }
}

/// A block of content in a tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// Text body.
        text: String,
        /// Annotations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<ContentAnnotations>,
        /// Extension metadata.
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<Map<String, Value>>,
    },
    /// Base64 image.
    Image {
        /// Base64 data.
        data: String,
        /// MIME type.
        #[serde(rename = "mimeType")]
        mime_type: String,
        /// Annotations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<ContentAnnotations>,
        /// Extension metadata.
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<Map<String, Value>>,
    },
    /// Base64 audio.
    Audio {
        /// Base64 data.
        data: String,
        /// MIME type.
        #[serde(rename = "mimeType")]
        mime_type: String,
        /// Annotations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<ContentAnnotations>,
        /// Extension metadata.
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<Map<String, Value>>,
    },
    /// A link to a resource.
    ResourceLink {
        /// Resource URI.
        uri: String,
        /// Resource name.
        name: String,
        /// Optional title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Optional description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// Optional MIME type.
        #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// Optional size.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
        /// Annotations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<ContentAnnotations>,
        /// Extension metadata.
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<Map<String, Value>>,
    },
    /// Embedded resource contents.
    Resource {
        /// The embedded resource.
        resource: ResourceContents,
        /// Annotations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<ContentAnnotations>,
        /// Extension metadata.
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<Map<String, Value>>,
    },
}

/// Result of `tools/call`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    /// Content blocks.
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    /// Structured result; may be any JSON value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    /// Whether the tool reported an error.
    #[serde(default)]
    pub is_error: bool,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

/// Content in the shape LLM APIs accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmContent {
    /// Plain text.
    Text {
        /// Text body.
        text: String,
    },
    /// Base64 image.
    Image {
        /// Base64 data.
        data: String,
        /// MIME type.
        mime_type: String,
    },
}

fn block_to_llm_content(block: &ContentBlock) -> LlmContent {
    match block {
        ContentBlock::Text { text, .. } => LlmContent::Text { text: text.clone() },
        ContentBlock::Image { data, mime_type, .. } => LlmContent::Image {
            data: data.clone(),
            mime_type: mime_type.clone(),
        },
        ContentBlock::Audio { mime_type, .. } => LlmContent::Text {
            text: format!("[audio {mime_type} omitted]"),
        },
        ContentBlock::ResourceLink { name, uri, .. } => LlmContent::Text {
            text: format!("{name}: {uri}"),
        },
        ContentBlock::Resource { resource, .. } => {
            if let Some(text) = resource.text() {
                return LlmContent::Text {
                    text: text.to_string(),
                };
            }
            let mime_type = resource.mime_type().unwrap_or("unknown type");
            if mime_type.starts_with("image/") {
                LlmContent::Image {
                    data: resource.blob().unwrap_or_default().to_string(),
                    mime_type: mime_type.to_string(),
                }
            } else {
                LlmContent::Text {
                    text: format!(
                        "[binary resource {} ({mime_type}) omitted]",
                        resource.uri()
                    ),
                }
            }
        }
    }
}

/// Convert a tool result to text and image content for a model.
///
/// Text and images pass through, embedded text resources become text,
/// embedded image resources become images, and other blocks become a short
/// text placeholder. A result without content blocks but with
/// `structuredContent` becomes its pretty-printed JSON.
pub fn to_llm_content(result: &CallToolResult) -> Vec<LlmContent> {
    let mut content: Vec<LlmContent> = result
        .content
        .iter()
        .map(block_to_llm_content)
        .collect();
    if content.is_empty() {
        if let Some(structured) = &result.structured_content {
            content.push(LlmContent::Text {
                text: serde_json::to_string_pretty(structured).unwrap_or_else(|_| "null".to_string()),
            });
        }
    }
    content
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn text_and_image_pass_through() {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "image", "data": "AA==", "mimeType": "image/png"}
            ]
        }))
        .unwrap();
        let content = to_llm_content(&result);
        assert_eq!(
            content[0],
            LlmContent::Text {
                text: "hello".to_string()
            }
        );
        assert_eq!(
            content[1],
            LlmContent::Image {
                data: "AA==".to_string(),
                mime_type: "image/png".to_string()
            }
        );
    }

    #[test]
    fn embedded_resources_convert() {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": [
                {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "body"}},
                {"type": "resource", "resource": {"uri": "file:///b.png", "mimeType": "image/png", "blob": "AA=="}},
                {"type": "resource", "resource": {"uri": "file:///c.bin", "mimeType": "application/octet-stream", "blob": "AA=="}}
            ]
        }))
        .unwrap();
        let content = to_llm_content(&result);
        assert!(matches!(&content[0], LlmContent::Text { text } if text == "body"));
        assert!(matches!(&content[1], LlmContent::Image { .. }));
        assert!(matches!(&content[2], LlmContent::Text { text } if text.contains("omitted")));
    }

    #[test]
    fn structured_fallback_when_content_empty() {
        let result = CallToolResult {
            content: Vec::new(),
            structured_content: Some(json!({"answer": 42})),
            ..Default::default()
        };
        let content = to_llm_content(&result);
        assert_eq!(content.len(), 1);
        match &content[0] {
            LlmContent::Text { text } => assert!(text.contains("\"answer\": 42")),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn structured_content_may_be_an_array() {
        let result: CallToolResult =
            serde_json::from_value(json!({"content": [], "structuredContent": [1, 2]})).unwrap();
        assert_eq!(result.structured_content, Some(json!([1, 2])));
    }
}
