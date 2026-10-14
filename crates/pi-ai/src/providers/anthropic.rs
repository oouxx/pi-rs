//! Anthropic Messages API provider.
//!
//! Thin wrapper around the Anthropic Messages API using reqwest for HTTP
//! and SSE streaming. Converts between pi-ai types and Anthropic API format.
//!
//! Ported from `packages/ai/src/providers/anthropic.ts`.

use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::types::{
    AnthropicMessagesCompat, AssistantMessage, AssistantMessageEvent, CacheRetention, ContentBlock,
    Context, Message, Model, ModelCompat, SimpleStreamOptions, StopReason, StreamOptions, Tool,
    Usage,
};
use crate::utils::event_stream::AssistantMessageEventStream;

// ============================================================================
// Constants
// ============================================================================

const ANTHROPIC_VERSION: &str = "2023-06-01";
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
#[allow(dead_code)]
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const MID_CONVERSATION_TOOL_CHANGES_BETA: &str = "mid-conversation-tool-changes-2026-07-01";
/// Advertises the server-side fallback models sent in `params.fallbacks`
/// (TS `SERVER_SIDE_FALLBACK_BETA`).
const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Substring that marks a Claude Code OAuth token (match TS `isOAuthToken`).
const OAUTH_TOKEN_MARKER: &str = "sk-ant-oat";
/// Claude Code CLI version advertised on OAuth requests (match TS `claudeCodeVersion`).
const CLAUDE_CODE_VERSION: &str = "2.1.251";
const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
const OAUTH_BETA: &str = "oauth-2025-04-20";

#[allow(dead_code)]
const CLAUDE_CODE_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

// ============================================================================
// Anthropic API types (request/response)
// ============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AnthropicMessageParam {
    role: String,
    content: AnthropicContent,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    String(String),
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "cache_control")]
        cache_control: Option<AnthropicCacheControl>,
    },
    #[serde(rename = "image")]
    Image {
        source: AnthropicImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "cache_control")]
        cache_control: Option<AnthropicCacheControl>,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_removal")]
    ToolRemoval { tool: AnthropicToolReference },
    #[serde(rename = "tool_addition")]
    ToolAddition { tool: AnthropicToolReference },
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(rename = "tool_use_id")]
        tool_use_id: String,
        content: AnthropicToolResultContent,
        #[serde(rename = "is_error", skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "cache_control")]
        cache_control: Option<AnthropicCacheControl>,
    },
}

/// tool_result 的 content：字符串或 block 数组（对齐 TS
/// `content: string | ContentBlockParam[]`，图片场景用数组）。
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum AnthropicToolResultContent {
    String(String),
    Blocks(Vec<AnthropicContentBlock>),
}

/// A tool reference inside a `tool_removal`/`tool_addition` block.
#[derive(Debug, Clone, Serialize)]
struct AnthropicToolReference {
    #[serde(rename = "type")]
    type_field: String,
    tool_name: String,
}

#[derive(Debug, Clone, Serialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: String,
    #[serde(rename = "media_type")]
    media_type: String,
    data: String,
}

/// Anthropic prompt-caching cache_control (match TS `CacheControlEphemeral`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnthropicCacheControl {
    #[serde(rename = "type")]
    cache_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl: Option<String>,
}

/// Resolve the anthropic prompt-caching cache_control from the retention
/// setting (match TS `getCacheControl`): `none` → no cache control; `long`
/// with `supportsLongCacheRetention` adds `ttl: "1h"`.
fn get_cache_control(
    model: &Model,
    options: Option<&StreamOptions>,
) -> Option<AnthropicCacheControl> {
    let retention = options.and_then(|o| o.cache_retention.as_ref());
    if matches!(retention, Some(CacheRetention::None)) {
        return None;
    }
    let long = retention == Some(&CacheRetention::Long)
        || (retention.is_none()
            && crate::env_api_keys::get_provider_env_value(
                "PI_CACHE_RETENTION",
                options.and_then(|o| o.env.as_ref()),
            )
            .as_deref()
                == Some("long"));
    let ttl = if long && get_anthropic_compat(model).supports_long_cache_retention.unwrap_or(false) {
        Some("1h".to_string())
    } else {
        None
    };
    Some(AnthropicCacheControl {
        cache_type: "ephemeral".to_string(),
        ttl,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "cache_control")]
    cache_control: Option<AnthropicCacheControl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "defer_loading")]
    defer_loading: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessageParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<AnthropicSystemPrompt>,
    max_tokens: u64,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthropicSystemPrompt {
    String(String),
    Blocks(Vec<Value>),
}

// SSE event types from Anthropic
#[derive(Debug, Deserialize)]
#[allow(clippy::enum_variant_names)]
#[serde(tag = "type")]
enum AnthropicSseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicMessageInfo },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: AnthropicDelta,
        usage: AnthropicUsage,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: AnthropicContentBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        index: usize,
        delta: AnthropicContentDelta,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageInfo {
    id: String,
    model: String,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "stop_reason")]
    stop_reason: Option<String>,
    #[serde(rename = "stop_details")]
    stop_details: Option<AnthropicStopDetails>,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicStopDetails {
    pub explanation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<AnthropicCacheCreation>,
    output_tokens_details: Option<AnthropicOutputTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct AnthropicCacheCreation {
    #[serde(rename = "ephemeral_1h_input_tokens")]
    ephemeral_1h_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AnthropicOutputTokensDetails {
    thinking_tokens: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[allow(clippy::enum_variant_names)]
#[serde(tag = "type")]
enum AnthropicContentBlockStart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)]
enum AnthropicContentDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

// ============================================================================
// Helper: compat resolution
// ============================================================================

fn get_anthropic_compat(model: &Model) -> AnthropicMessagesCompat {
    match &model.compat {
        Some(ModelCompat::AnthropicMessages(compat)) => compat.clone(),
        _ => AnthropicMessagesCompat {
            allowed_fallback_models: None,
            supports_eager_tool_input_streaming: None,
            supports_long_cache_retention: None,
            send_session_affinity_headers: None,
            supports_cache_control_on_tools: None,
            allow_empty_signature: None,
            force_adaptive_thinking: None,
            supports_strict_tools: None,
            supports_mid_convo_system_messages: None,
            supports_mid_convo_tool_changes: None,
            supports_mid_convo_effort: None,
        },
    }
}

// ============================================================================
// Message conversion (pi-ai → Anthropic API format)
// ============================================================================

/// Normalize a tool call ID to Anthropic's requirements (alphanumeric + _- only, max 64 chars).
fn normalize_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// 对齐 TS `convertContentBlocks`：纯文本 → 拼接字符串；有图 → block
/// 数组（text + image），纯图无文本时前插 `"(see attached image)"`。
fn convert_content_blocks(content: &[ContentBlock]) -> AnthropicToolResultContent {
    let has_images = content.iter().any(|b| matches!(b, ContentBlock::Image { .. }));
    if !has_images {
        let text = content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return AnthropicToolResultContent::String(text);
    }
    let mut blocks: Vec<AnthropicContentBlock> = Vec::new();
    let mut has_text = false;
    for b in content {
        match b {
            ContentBlock::Text { text, .. } => {
                has_text = true;
                blocks.push(AnthropicContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                });
            }
            ContentBlock::Image { data, mime_type } => {
                blocks.push(AnthropicContentBlock::Image {
                    source: AnthropicImageSource {
                        source_type: "base64".to_string(),
                        media_type: mime_type.clone(),
                        data: data.clone(),
                    },
                    cache_control: None,
                });
            }
            _ => {}
        }
    }
    if !has_text {
        blocks.insert(
            0,
            AnthropicContentBlock::Text {
                text: "(see attached image)".to_string(),
                cache_control: None,
            },
        );
    }
    AnthropicToolResultContent::Blocks(blocks)
}

/// Convert pi-ai messages to Anthropic API message params.
#[cfg(test)]
pub(crate) fn convert_messages(
    messages: &[Message],
    model: &Model,
    cache_control: Option<&AnthropicCacheControl>,
    native_system_messages: bool,
) -> Vec<AnthropicMessageParam> {
    convert_messages_full(messages, model, cache_control, native_system_messages, false)
}

pub(crate) fn convert_messages_full(
    messages: &[Message],
    model: &Model,
    cache_control: Option<&AnthropicCacheControl>,
    native_system_messages: bool,
    native_tool_changes: bool,
) -> Vec<AnthropicMessageParam> {
    let allow_empty_signature = get_anthropic_compat(model)
        .allow_empty_signature
        .unwrap_or(false);
    let mut params: Vec<AnthropicMessageParam> = Vec::new();
    // Later system messages are held back and emitted directly before the next
    // assistant message (or at the end). Anthropic requires `tool_result` blocks
    // to immediately follow their `tool_use`, so a system message between them
    // is rejected (TS `pendingSystemMessages`).
    let mut pending_system_messages: Vec<AnthropicMessageParam> = Vec::new();

    let mut i = 0usize;
    while i < messages.len() {
        let msg = &messages[i];
        match msg {
            Message::User { content, .. } => {
                // 对齐 TS convertMessages 用户分支：逐消息 push（不替换
                // 上一条），文本块 trim 后为空则剔除，全部为空则整条跳过。
                let mut blocks: Vec<AnthropicContentBlock> = Vec::new();
                let mut has_images = false;
                for b in content {
                    match b {
                        ContentBlock::Text { text, .. } => {
                            if text.trim().is_empty() {
                                continue;
                            }
                            blocks.push(AnthropicContentBlock::Text {
                                text: text.clone(),
                                cache_control: None,
                            });
                        }
                        ContentBlock::Image { data, mime_type } => {
                            has_images = true;
                            blocks.push(AnthropicContentBlock::Image {
                                source: AnthropicImageSource {
                                    source_type: "base64".to_string(),
                                    media_type: mime_type.clone(),
                                    data: data.clone(),
                                },
                                cache_control: None,
                            });
                        }
                        _ => {}
                    }
                }
                if blocks.is_empty() {
                    i += 1;
                    continue;
                }
                // 纯文本（无图）→ 拼接字符串；有图 → block 数组。
                let content_param = if has_images {
                    AnthropicContent::Blocks(blocks)
                } else {
                    let text = blocks
                        .iter()
                        .filter_map(|b| match b {
                            AnthropicContentBlock::Text { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    AnthropicContent::String(text)
                };
                params.push(AnthropicMessageParam {
                    role: "user".to_string(),
                    content: content_param,
                });
            }
            Message::Assistant { content, .. } => {
                // Pending system updates land right before the next assistant
                // (TS flushPendingSystemMessages).
                params.append(&mut pending_system_messages);
                let mut blocks: Vec<AnthropicContentBlock> = Vec::new();
                for b in content {
                    match b {
                        ContentBlock::Text { text, .. } => {
                            if text.trim().is_empty() {
                                continue;
                            }
                            blocks.push(AnthropicContentBlock::Text {
                                text: text.clone(),
                                cache_control: None,
                            });
                        }
                        ContentBlock::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                        } => {
                            // Redacted thinking: pass the opaque payload back as
                            // redacted_thinking (mirrors TS convertMessages).
                            if redacted.unwrap_or(false) {
                                if let Some(sig) = thinking_signature {
                                    blocks.push(AnthropicContentBlock::RedactedThinking {
                                        data: sig.clone(),
                                    });
                                }
                                continue;
                            }
                            let has_signature = thinking_signature
                                .as_deref()
                                .map(|s| !s.trim().is_empty())
                                .unwrap_or(false);
                            if thinking.trim().is_empty() && !has_signature {
                                continue;
                            }
                            // Missing/empty signature (e.g. aborted stream): convert to
                            // plain text, unless the model is marked to accept empty
                            // signatures (mirrors TS convertMessages).
                            if !has_signature {
                                if allow_empty_signature {
                                    blocks.push(AnthropicContentBlock::Thinking {
                                        thinking: thinking.clone(),
                                        signature: String::new(),
                                    });
                                } else {
                                    blocks.push(AnthropicContentBlock::Text {
                                        text: thinking.clone(),
                                        cache_control: None,
                                    });
                                }
                            } else {
                                blocks.push(AnthropicContentBlock::Thinking {
                                    thinking: thinking.clone(),
                                    signature: thinking_signature.clone().unwrap_or_default(),
                                });
                            }
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => blocks.push(AnthropicContentBlock::ToolUse {
                            id: normalize_tool_call_id(id),
                            name: name.clone(),
                            input: arguments.clone(),
                        }),
                        ContentBlock::Image { .. } => {}
                    }
                }

                if !blocks.is_empty() {
                    params.push(AnthropicMessageParam {
                        role: "assistant".to_string(),
                        content: AnthropicContent::Blocks(blocks),
                    });
                }
            }
            Message::ToolResult { .. } => {
                // 对齐 TS convertMessages toolResult 分支：合并连续
                // toolResult 消息为一条 user 消息（z.ai 端点要求）。
                let mut tool_results: Vec<AnthropicContentBlock> = Vec::new();
                while i < messages.len() {
                    let Message::ToolResult {
                        tool_call_id,
                        content,
                        is_error,
                        ..
                    } = &messages[i]
                    else {
                        break;
                    };
                    // 对齐 TS convertToolResult：tool_result 只带内容；
                    // tool reference 仅出现在 native tool changes 的
                    // tool_removal/tool_addition block 里。
                    tool_results.push(AnthropicContentBlock::ToolResult {
                        tool_use_id: tool_call_id.clone(),
                        content: convert_content_blocks(content),
                        is_error: if *is_error { Some(true) } else { None },
                        cache_control: None,
                    });
                    i += 1;
                }
                let content = tool_results;
                params.push(AnthropicMessageParam {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(content),
                });
                continue;
            }
            // Later system messages only reach this point on models that accept
            // them natively; otherwise the transcript is collapsed first.
            Message::System { .. } => {
                if native_system_messages {
                    let text =
                        crate::utils::transcript::typed_render_system_message_update(msg);
                    let mut blocks: Vec<AnthropicContentBlock> = Vec::new();
                    if !text.is_empty() {
                        blocks.push(AnthropicContentBlock::Text {
                            text,
                            cache_control: None,
                        });
                    }
                    if native_tool_changes {
                        if let Message::System {
                            tools_removed,
                            tools_added,
                            ..
                        } = msg
                        {
                            for tool in tools_removed.iter().flatten() {
                                blocks.push(AnthropicContentBlock::ToolRemoval {
                                    tool: AnthropicToolReference {
                                        type_field: "tool_reference".to_string(),
                                        tool_name: tool.name.clone(),
                                    },
                                });
                            }
                            for tool in tools_added.iter().flatten() {
                                blocks.push(AnthropicContentBlock::ToolAddition {
                                    tool: AnthropicToolReference {
                                        type_field: "tool_reference".to_string(),
                                        tool_name: tool.name.clone(),
                                    },
                                });
                            }
                        }
                    }
                    if !blocks.is_empty() {
                        pending_system_messages.push(AnthropicMessageParam {
                            role: "system".to_string(),
                            content: AnthropicContent::Blocks(blocks),
                        });
                    }
                }
            }
        }
        i += 1;
    }

    // Add cache_control to the last user message to cache conversation history
    // (match TS convertMessages).
    if let Some(cc) = cache_control {
        if let Some(last) = params.last_mut() {
            if last.role == "user" {
                match &mut last.content {
                    AnthropicContent::String(s) => {
                        let s = s.clone();
                        last.content = AnthropicContent::Blocks(vec![AnthropicContentBlock::Text {
                            text: s,
                            cache_control: Some(cc.clone()),
                        }]);
                    }
                    AnthropicContent::Blocks(blocks) => {
                        if let Some(
                            AnthropicContentBlock::Text { cache_control: c, .. }
                            | AnthropicContentBlock::Image { cache_control: c, .. }
                            | AnthropicContentBlock::ToolResult { cache_control: c, .. },
                        ) = blocks.last_mut()
                        {
                            *c = Some(cc.clone());
                        }
                    }
                }
            }
        }
    }

    params.append(&mut pending_system_messages);
    params
}

// ============================================================================
// Tool conversion
// ============================================================================

/// Convert pi-ai tools to Anthropic API tool definitions.
pub(crate) fn convert_tools(
    tools: &[Tool],
    cache_control: Option<&AnthropicCacheControl>,
    defer_loading: bool,
    supports_strict_tools: bool,
) -> Result<Vec<AnthropicTool>, String> {
    let count = tools.len();
    tools
        .iter()
        .enumerate()
        .map(|(index, t)| {
            // TS 0.84.2: strict tools get provider-compatible closed-object
            // schemas (every property required, optional non-nullable widened
            // to nullable) while the original tool definition stays untouched.
            let strict = crate::providers::openai_responses::resolve_json_schema_strict_sampling(
                t,
                supports_strict_tools,
            )?;
            let parameters = crate::utils::strict_schema::get_json_schema_tool_parameters(
                &t.parameters,
                strict,
            )?;
            let schema = parameters.as_object().cloned().unwrap_or_default();
            // TS `legacyInputSchema`: always send the minimal object form.
            let mut legacy_input_schema = serde_json::Map::new();
            legacy_input_schema.insert("type".into(), json!("object"));
            legacy_input_schema.insert(
                "properties".into(),
                schema
                    .get("properties")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            );
            legacy_input_schema.insert(
                "required".into(),
                schema.get("required").cloned().unwrap_or_else(|| json!([])),
            );
            let input_schema = if strict == Some(true) {
                // Strict: full converted parameters, with the legacy fields
                // taking precedence (TS spread order).
                let mut merged = parameters.as_object().cloned().unwrap_or_default();
                for (k, v) in legacy_input_schema {
                    merged.insert(k, v);
                }
                Value::Object(merged)
            } else {
                Value::Object(legacy_input_schema)
            };
            Ok(AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema,
                cache_control: if index == count - 1 {
                    cache_control.cloned()
                } else {
                    None
                },
                defer_loading: if defer_loading { Some(true) } else { None },
                strict,
            })
        })
        .collect()
}

// ============================================================================
// Stop reason mapping
// ============================================================================

/// Map Anthropic stop reason to pi-ai `StopReason`.
#[must_use]
/// Map an Anthropic stop reason to a pi stop reason, plus an optional error
/// message for error-class stops.
///
/// Mirrors `mapStopReason` in `packages/ai/src/api/anthropic-messages.ts`:
/// `refusal` becomes an error carrying the provider's `stop_details.explanation`,
/// `sensitive` becomes an error, and unknown reasons are surfaced instead of
/// being silently treated as a normal stop.
pub fn map_stop_reason(
    reason: &str,
    stop_details: Option<&AnthropicStopDetails>,
) -> (StopReason, Option<String>) {
    match reason {
        "end_turn" => (StopReason::Stop, None),
        "max_tokens" => (StopReason::Length, None),
        "tool_use" => (StopReason::ToolUse, None),
        "refusal" => (
            StopReason::Error,
            Some(
                stop_details
                    .and_then(|d| d.explanation.clone())
                    .unwrap_or_else(|| "The model refused to complete the request".to_string()),
            ),
        ),
        "pause_turn" => (StopReason::Stop, None),
        "stop_sequence" => (StopReason::Stop, None),
        "sensitive" => (
            StopReason::Error,
            Some("Provider stopped with: sensitive".to_string()),
        ),
        // Handle unknown stop reasons gracefully (API may add new values),
        // matching TS which throws — here surfaced as an error stop reason
        // instead of panicking the stream task.
        other => (
            StopReason::Error,
            Some(format!("Unhandled stop reason: {other}")),
        ),
    }
}

// ============================================================================
// Cache control resolution
// ============================================================================

// ============================================================================
// StreamAnthropic: main streaming function
// ============================================================================

/// Resolved Anthropic request credential (match TS `anthropicApiKeyAuth().resolve`).
#[derive(Debug, PartialEq, Eq)]
struct AnthropicAuth {
    /// `apiKey` returned by auth resolution (may itself be an OAuth token).
    api_key: Option<String>,
    /// `true` when `ANTHROPIC_AUTH_TOKEN` supplies the Bearer credential.
    use_bearer: bool,
    /// The `ANTHROPIC_AUTH_TOKEN` value when it is the active credential.
    auth_token: Option<String>,
}

/// Resolve the Anthropic request credential from explicit/stored key, the
/// gateway bearer token, and the OAuth/API-key env fallbacks, in TS order:
/// explicit key → `ANTHROPIC_AUTH_TOKEN` → `ANTHROPIC_OAUTH_TOKEN` → `ANTHROPIC_API_KEY`.
fn resolve_anthropic_auth(
    explicit_api_key: Option<String>,
    auth_token: Option<String>,
    oauth_token: Option<String>,
    api_key_env: Option<String>,
) -> AnthropicAuth {
    if let Some(key) = explicit_api_key.filter(|k| !k.is_empty()) {
        return AnthropicAuth {
            api_key: Some(key),
            use_bearer: false,
            auth_token: None,
        };
    }
    if let Some(token) = auth_token {
        return AnthropicAuth {
            api_key: None,
            use_bearer: true,
            auth_token: Some(token),
        };
    }
    AnthropicAuth {
        api_key: oauth_token.or(api_key_env),
        use_bearer: false,
        auth_token: None,
    }
}

/// `sk-ant-oat…` marks a Claude Code OAuth token (match TS `isOAuthToken`).
fn is_oauth_token(api_key: Option<&str>) -> bool {
    api_key.is_some_and(|key| key.contains(OAUTH_TOKEN_MARKER))
}

/// Auth-related headers for an Anthropic request (match TS `createClient`).
/// GitHub Copilot and `ANTHROPIC_AUTH_TOKEN` use `Authorization: Bearer`;
/// `sk-ant-oat…` OAuth tokens also add Claude Code identity headers.
/// Returns `(name, value)` pairs so the result is order-stable for tests.
fn anthropic_auth_headers(
    provider: &str,
    api_key: Option<&str>,
    auth_token: Option<&str>,
    use_bearer: bool,
) -> Vec<(String, String)> {
    if use_bearer {
        return auth_token.map_or_else(Vec::new, |token| {
            vec![("authorization".to_string(), format!("Bearer {token}"))]
        });
    }
    let key = api_key.unwrap_or_default();
    // Copilot: Bearer auth, no Claude Code identity headers.
    if provider == "github-copilot" {
        return vec![("authorization".to_string(), format!("Bearer {key}"))];
    }
    if is_oauth_token(api_key) {
        // OAuth: Bearer auth plus Claude Code identity headers.
        vec![
            ("authorization".to_string(), format!("Bearer {key}")),
            (
                "user-agent".to_string(),
                format!("claude-cli/{CLAUDE_CODE_VERSION}"),
            ),
            ("x-app".to_string(), "cli".to_string()),
        ]
    } else {
        vec![("x-api-key".to_string(), key.to_string())]
    }
}

/// Resolve the Anthropic request credential for the model's provider.
///
/// Only the `anthropic` provider owns the `ANTHROPIC_AUTH_TOKEN` /
/// `ANTHROPIC_OAUTH_TOKEN` / `ANTHROPIC_API_KEY` variables; other providers
/// that speak the Anthropic wire format (e.g. `minimax`, `fireworks`,
/// `github-copilot`) fall back to their own env var.
fn resolve_anthropic_provider_auth(
    provider: &str,
    explicit_api_key: Option<String>,
    auth_token: Option<String>,
    oauth_token: Option<String>,
    anthropic_api_key: Option<String>,
    provider_env_key: Option<String>,
) -> AnthropicAuth {
    if provider == "anthropic" {
        resolve_anthropic_auth(explicit_api_key, auth_token, oauth_token, anthropic_api_key)
    } else {
        AnthropicAuth {
            api_key: explicit_api_key
                .filter(|k| !k.is_empty())
                .or(provider_env_key),
            use_bearer: false,
            auth_token: None,
        }
    }
}

/// Stream a completion from the Anthropic Messages API.
#[must_use]
pub fn stream_anthropic(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> AssistantMessageEventStream {
    let model = model.clone();
    // Native mid-convo models keep the transcript system messages; others fold
    // them into the leading prompt/tools (P1-7).
    let supports_mid_convo = get_anthropic_compat(&model)
        .supports_mid_convo_system_messages
        .unwrap_or(false);
    let context = if supports_mid_convo {
        context.clone()
    } else {
        context.collapsed()
    };
    let owned_options = options.cloned();
    // Auth resolution (match TS `anthropicApiKeyAuth().resolve` for the
    // `anthropic` provider):
    // 1. explicit/stored credential api_key → apiKey
    // 2. ANTHROPIC_AUTH_TOKEN → Authorization: Bearer
    // 3. ANTHROPIC_OAUTH_TOKEN / ANTHROPIC_API_KEY env → apiKey
    // An OAuth token (`sk-ant-oat…`) still travels as `apiKey` here and is
    // upgraded to Bearer by the request layer, matching TS `isOAuthToken`.
    let explicit_api_key = owned_options.as_ref().and_then(|o| o.api_key.clone());
    let auth = resolve_anthropic_provider_auth(
        &model.provider,
        explicit_api_key,
        crate::env_api_keys::get_provider_env_value(
            crate::env_api_keys::ANTHROPIC_AUTH_TOKEN_ENV,
            None,
        ),
        crate::env_api_keys::get_provider_env_value(
            crate::env_api_keys::ANTHROPIC_OAUTH_TOKEN_ENV,
            None,
        ),
        crate::env_api_keys::get_provider_env_value(
            crate::env_api_keys::ANTHROPIC_API_KEY_ENV,
            None,
        ),
        crate::env_api_keys::get_env_api_key(&model.provider, None),
    );
    let AnthropicAuth {
        api_key,
        use_bearer,
        auth_token,
    } = auth;
    let auth_token_for_inner = auth_token.clone();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        let result = stream_anthropic_inner(
            &model,
            &context,
            owned_options.as_ref(),
            api_key.as_deref(),
            use_bearer,
            auth_token_for_inner.as_deref(),
            &tx,
        )
        .await;
        if let Err(e) = result {
            let _ = tx.send(AssistantMessageEvent::Error {
                reason: StopReason::Error,
                error: AssistantMessage {
                    content: vec![],
                    api: model.api.clone(),
                    provider: model.provider.clone(),
                    model: model.id.clone(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::Error,
                    error_message: Some(e.to_string()),
                    raw_stop_reason: None,
                    end_turn: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            });
        }
    });

    AssistantMessageEventStream::from_receiver(rx)
}

async fn stream_anthropic_inner(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
    api_key: Option<&str>,
    use_bearer: bool,
    auth_token: Option<&str>,
    tx: &tokio::sync::mpsc::UnboundedSender<AssistantMessageEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // When using ANTHROPIC_AUTH_TOKEN (Bearer), no x-api-key is required.
    let api_key = if use_bearer {
        api_key.map(|s| s.to_string())
    } else {
        Some(
            api_key
                .ok_or_else(|| format!("No API key for provider: {}", model.provider))?
                .to_string(),
        )
    };

    let compat = get_anthropic_compat(model);
    let supports_mid_convo = compat.supports_mid_convo_system_messages.unwrap_or(false);
    // Tools come from the transcript (TS `getCurrentTools(context.messages)`);
    // the request-level shorthand is only filled when the transcript was
    // collapsed for a model without mid-conversation system messages.
    let request_tools =
        crate::utils::transcript::request_tools(&context.messages, context.tools.as_deref());
    // Native tool changes (TS `nativeToolChanges`): only when the model accepts
    // mid-convo system messages and tool changes, has an initial active tool,
    // and no tool name is redefined.
    let (native_initial_tools, native_later_tools, native_tool_changes) = if supports_mid_convo
        && compat.supports_mid_convo_tool_changes.unwrap_or(false)
    {
        let values: Vec<Value> = serde_json::to_value(&context.messages)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let initial: Vec<crate::types::Tool> = match context.messages.first() {
            Some(Message::System {
                tools_added: Some(tools),
                ..
            }) => tools.clone(),
            _ => Vec::new(),
        };
        let initial_names: std::collections::HashSet<String> =
            initial.iter().map(|t| t.name.clone()).collect();
        let later: Vec<crate::types::Tool> = crate::utils::transcript::get_declared_tools(&values)
            .into_iter()
            .filter(|t| !initial_names.contains(&t.name))
            .collect();
        let active = !initial.is_empty()
            && !crate::utils::transcript::has_tool_redefinitions(&values);
        (initial, later, active)
    } else {
        (Vec::new(), Vec::new(), false)
    };
    let temperature = options.and_then(|o| o.temperature);
    let max_tokens = options
        .and_then(|o| o.max_tokens)
        .unwrap_or(model.max_tokens);
    let signal = options.and_then(|o| o.signal.clone());
    // Idle timeout (TS `httpIdleTimeoutMs` → undici `bodyTimeout`).
    let idle_timeout = crate::utils::idle::resolve_idle_timeout(options.and_then(|o| o.timeout_ms));
    let _cache_retention = options.and_then(|o| o.cache_retention.as_ref());

    // Allow extensions to modify HTTP request headers
    let mut header_map = std::collections::HashMap::new();
    for (name, value) in anthropic_auth_headers(
        &model.provider,
        api_key.as_deref(),
        auth_token,
        use_bearer,
    ) {
        header_map.insert(name, value);
    }
    header_map.insert(
        "anthropic-version".to_string(),
        ANTHROPIC_VERSION.to_string(),
    );
    header_map.insert("content-type".to_string(), "application/json".to_string());
    // Static catalog headers (match TS createClient `model.headers`).
    if let Some(model_headers) = &model.headers {
        for (key, value) in model_headers {
            header_map.insert(key.clone(), value.clone());
        }
    }
    // Request-scoped headers override the above (match TS `optionsHeaders`).
    if let Some(option_headers) = options.and_then(|o| o.headers.as_ref()) {
        for (key, value) in option_headers {
            header_map.insert(key.clone(), value.clone());
        }
    }
    // Fine-grained tool streaming beta: only when tools are present AND the
    // provider does not support eager tool input streaming (match TS
    // `shouldUseFineGrainedToolStreamingBeta`).
    let has_tools = !request_tools.is_empty();
    let mut beta_features: Vec<String> = Vec::new();
    // Claude Code OAuth requests advertise the identity betas (match TS
    // `getBetaFeatures`); Copilot auth is Bearer but not OAuth.
    let is_oauth = !use_bearer
        && model.provider != "github-copilot"
        && is_oauth_token(api_key.as_deref());
    if is_oauth {
        beta_features.push(CLAUDE_CODE_BETA.to_string());
        beta_features.push(OAUTH_BETA.to_string());
    }
    if has_tools && !compat.supports_eager_tool_input_streaming.unwrap_or(true) {
        beta_features.push(FINE_GRAINED_TOOL_STREAMING_BETA.to_string());
    }
    // Interleaved thinking beta for non-adaptive thinking models
    // (match TS createClient `needsInterleavedBeta`).
    if compat.force_adaptive_thinking != Some(true) {
        beta_features.push(INTERLEAVED_THINKING_BETA.to_string());
    }
    if native_tool_changes {
        beta_features.push(MID_CONVERSATION_TOOL_CHANGES_BETA.to_string());
    }
    let allowed_fallback_models = compat.allowed_fallback_models.clone().unwrap_or_default();
    if !allowed_fallback_models.is_empty() {
        beta_features.push(SERVER_SIDE_FALLBACK_BETA.to_string());
    }
    if !beta_features.is_empty() {
        header_map.insert(
            "anthropic-beta".to_string(),
            beta_features.join(","),
        );
    }
    if let Some(session_id) = options.and_then(|o| o.session_id.as_deref()) {
        if compat.send_session_affinity_headers.unwrap_or(false) {
            header_map.insert("x-session-affinity".to_string(), session_id.to_string());
        }
    }

    let final_headers =
        if let Some(on_headers) = options.as_ref().and_then(|o| o.on_headers.as_ref()) {
            on_headers(header_map).await
        } else {
            header_map
        };

    // Per-request HTTP client injection (TS per-request `fetch`); headers are
    // applied per request so an injected client is honored.
    let http_client = match options.and_then(|o| o.http_client.clone()) {
        Some(client) => (*client).clone(),
        None => HttpClient::builder().build()?,
    };
    let header_pairs: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)> = final_headers
        .iter()
        .filter_map(|(key, value)| {
            Some((
                reqwest::header::HeaderName::from_bytes(key.as_bytes()).ok()?,
                reqwest::header::HeaderValue::from_str(value).ok()?,
            ))
        })
        .collect();

    // Build request body
    let cache_control = get_cache_control(model, options);
    // 对齐 TS buildParams：先 transformMessages（图片降级/thinking 预处理/
    // tool call id 归一化/孤儿合成/跳过 errored-aborted），再 convertMessages。
    let transformed = crate::utils::transform::transform_messages(
        &context.messages,
        model,
        &|id: &str, _m: &Model, _p: &str, _a: &str| normalize_tool_call_id(id),
    );
    // Native mid-convo: the leading system message carries the prompt and later
    // system messages are sent in place; otherwise the transcript was collapsed
    // into `context.system_prompt`.
    let (initial_system_text, conversation): (Option<String>, &[Message]) = if supports_mid_convo {
        match transformed.first() {
            Some(m @ Message::System { .. }) => {
                let text = crate::utils::transcript::typed_system_message_text(m);
                (
                    if text.is_empty() { None } else { Some(text) },
                    &transformed[1..],
                )
            }
            _ => (context.system_prompt.clone(), &transformed[..]),
        }
    } else {
        (context.system_prompt.clone(), &transformed[..])
    };
    // TS buildParams declares the current tool list plainly outside the native
    // tool-changes branch: the request-level deferred split was removed in
    // #9548, and tool references only appear in `tool_removal`/`tool_addition`
    // blocks.
    let messages = convert_messages_full(
        conversation,
        model,
        cache_control.as_ref(),
        supports_mid_convo,
        native_tool_changes,
    );
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(model.id.clone()));
    body.insert("messages".to_string(), serde_json::to_value(&messages)?);
    body.insert("max_tokens".to_string(), Value::Number(max_tokens.into()));
    body.insert("stream".to_string(), Value::Bool(true));

    if let Some(ref sp) = initial_system_text {
        // System prompt with anthropic cache_control (match TS buildParams).
        let mut system_text = serde_json::Map::new();
        system_text.insert("type".into(), json!("text"));
        system_text.insert("text".into(), json!(sp));
        if let Some(cc) = &cache_control {
            system_text.insert("cache_control".into(), serde_json::to_value(cc)?);
        }
        body.insert("system".to_string(), json!([system_text]));
    }

    if let Some(t) = temperature {
        body.insert(
            "temperature".to_string(),
            Value::Number(
                serde_json::Number::from_f64(t)
                    .ok_or_else(|| format!("Invalid temperature: {t}"))?,
            ),
        );
    }

    let tools_cache_control = if compat.supports_cache_control_on_tools.unwrap_or(false) {
        cache_control.as_ref()
    } else {
        None
    };
    let supports_strict_tools = compat.supports_strict_tools.unwrap_or(false);
    if native_tool_changes {
        // Initial tools stay active with the cache breakpoint on the last one;
        // later declarations are deferred (only surfaced by their
        // `tool_addition` block), and removed tools are withdrawn by
        // `tool_removal`. The request-level list only grows, keeping the cached
        // prefix intact (TS native tool changes).
        let mut all_tools = convert_tools(
            &native_initial_tools,
            tools_cache_control,
            false,
            supports_strict_tools,
        )?;
        all_tools.push(AnthropicTool {
            name: "__pi_deferred_placeholder__".to_string(),
            description: "Reserved placeholder. Never available. Never call this."
                .to_string(),
            input_schema: json!({"type": "object", "properties": {}, "required": []}),
            cache_control: None,
            defer_loading: Some(true),
            strict: None,
        });
        all_tools.extend(convert_tools(
            &native_later_tools,
            None,
            true,
            supports_strict_tools,
        )?);
        body.insert("tools".to_string(), serde_json::to_value(all_tools)?);
    } else if !request_tools.is_empty() {
        // cache_control on the last tool (match TS convertTools) only when the
        // provider supports it.
        let all_tools = convert_tools(
            &request_tools,
            tools_cache_control,
            false,
            supports_strict_tools,
        )?;
        body.insert("tools".to_string(), serde_json::to_value(all_tools)?);
    }

    if !allowed_fallback_models.is_empty() {
        // Server-side fallbacks are advertised as model ids only; the
        // provider decides when to use them (TS `params.fallbacks`).
        let fallbacks: Vec<Value> = allowed_fallback_models
            .iter()
            .map(|fallback| json!({ "model": fallback.model }))
            .collect();
        body.insert("fallbacks".to_string(), Value::Array(fallbacks));
    }

    // Configure thinking mode: adaptive, budget-based, or explicitly disabled
    // (match TS buildParams). pi-rs maps the reasoning effort level to the
    // TS `thinkingEnabled` semantics: presence enables thinking, "off" disables.
    if model.reasoning {
        let effort = options.and_then(|o| o.reasoning_effort.clone());
        match effort.as_deref() {
            Some("off") => {
                if model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|m| m.get("off").and_then(Option::as_deref))
                    .is_some()
                {
                    body.insert("thinking".into(), json!({ "type": "disabled" }));
                }
            }
            Some(level) => {
                if compat.force_adaptive_thinking == Some(true) {
                    body.insert(
                        "thinking".into(),
                        json!({ "type": "adaptive", "display": "summarized" }),
                    );
                    body.insert("output_config".into(), json!({ "effort": level }));
                } else {
                    // Budget-based thinking (match TS `streamSimple`): adjust
                    // maxTokens for the thinking budget and clamp it to the
                    // model's context window.
                    let adjusted =
                        crate::providers::simple_options::adjust_max_tokens_for_thinking(
                            options.and_then(|o| o.max_tokens),
                            model.max_tokens,
                            level,
                            options.and_then(|o| o.thinking_budgets.as_ref()),
                        );
                    let effective_max_tokens =
                        crate::providers::simple_options::clamp_max_tokens_to_context(
                            model,
                            context,
                            adjusted.max_tokens,
                        );
                    let budget =
                        crate::providers::simple_options::clamp_thinking_budget_to_answer_room(
                            adjusted.thinking_budget,
                            effective_max_tokens,
                        );
                    body.insert(
                        "max_tokens".into(),
                        Value::Number(effective_max_tokens.into()),
                    );
                    body.insert(
                        "thinking".into(),
                        json!({
                            "type": "enabled",
                            "budget_tokens": budget,
                            "display": "summarized",
                        }),
                    );
                }
            }
            None => {}
        }
    }

    // tool_choice (match TS buildParams).
    if let Some(tc) = options.and_then(|o| o.tool_choice.as_ref()) {
        match tc {
            crate::types::ToolChoice::Mode(m) => {
                let type_str = match m {
                    crate::types::ToolChoiceMode::Auto => "auto",
                    crate::types::ToolChoiceMode::None => "none",
                    crate::types::ToolChoiceMode::Required => "any",
                };
                body.insert("tool_choice".into(), json!({ "type": type_str }));
            }
            crate::types::ToolChoice::Specific { function, .. } => {
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                body.insert("tool_choice".into(), json!({ "type": "tool", "name": name }));
            }
        }
    }

    // metadata.user_id (match TS buildParams).
    if let Some(meta) = options.and_then(|o| o.metadata.as_ref()) {
        if let Some(uid) = meta.get("user_id").and_then(Value::as_str) {
            body.insert("metadata".into(), json!({ "user_id": uid }));
        }
    }

    // Check for abort signal before making the request
    if let Some(ref rx) = signal {
        if *rx.borrow() {
            return Err("Request was aborted".into());
        }
    }

    let request_body = Value::Object(body);

    // Allow extensions to modify the request payload
    let request_body = if let Some(on_payload) = options.and_then(|o| o.on_payload.as_ref()) {
        match on_payload(request_body).await {
            Some(modified) => modified,
            None => return Err("Request cancelled by extension".into()),
        }
    } else {
        request_body
    };

    let response = {
        let mut request = http_client.post(&model.base_url).json(&request_body);
        for (key, value) in &header_pairs {
            request = request.header(key.clone(), value.clone());
        }
        // HTTP-level retry (match TS `retryProviderRequest`): transient errors
        // are retried with exponential backoff; the abort signal interrupts.
        crate::utils::provider_retry::send_with_retry(
            request.build()?,
            &http_client,
            signal.clone(),
            options.and_then(|o| o.max_retries),
            options.and_then(|o| o.max_retry_delay_ms),
            idle_timeout,
            "Anthropic API error",
        )
        .await
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?
    };

    // Notify extensions about the provider response
    if let Some(on_provider_response) = options
        .as_ref()
        .and_then(|o| o.on_provider_response.as_ref())
    {
        let status = response.status().as_u16();
        let resp_headers: std::collections::HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        on_provider_response(status, resp_headers);
    }

    // Stream the SSE body incrementally (match TS `iterateSseMessages`).
    let events = crate::utils::sse::sse_events_stream(
        response.bytes_stream().map(|item| item.map(|b| b.to_vec())),
    );
    futures::pin_mut!(events);

    // Initialize output
    // Set when a server-side fallback model answered: pricing then follows the
    // fallback's own rates (TS `usageModel`).
    let mut usage_model_cost: Option<crate::types::ModelCost> = None;
    let mut output = AssistantMessage {
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Pending,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    };

    let _ = tx.send(AssistantMessageEvent::Start {
        partial: output.clone(),
    });

    // Track content blocks with their indices
    #[derive(Debug, Clone)]
    struct BlockInfo {
        block: ContentBlock,
        index: usize,
        partial_json: String,
    }

    let mut blocks: Vec<BlockInfo> = Vec::new();

    use futures::StreamExt;

    // Abort future: completes when the abort signal flips to true (matching TS,
    // where the fetch is passed the AbortSignal and aborts actively). With no
    // signal it never completes.
    let abort_fut = async {
        if let Some(mut rx) = signal.clone() {
            loop {
                if *rx.borrow() {
                    break;
                }
                if rx.changed().await.is_err() {
                    break;
                }
            }
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(abort_fut);

    loop {
        let next = tokio::select! {
            res = crate::utils::idle::with_idle_timeout(events.next(), idle_timeout) => match res {
                Ok(sse) => sse,
                Err(()) => {
                    output.stop_reason = StopReason::Error;
                    output.error_message = Some(
                        idle_timeout
                            .map(crate::utils::idle::idle_timeout_message)
                            .unwrap_or_else(|| "Request timed out".to_string()),
                    );
                    let _ = tx.send(AssistantMessageEvent::Error {
                        reason: StopReason::Error,
                        error: output.clone(),
                    });
                    return Ok(());
                }
            },
            _ = &mut abort_fut => {
                output.stop_reason = StopReason::Aborted;
                output.error_message = Some("Request was aborted".to_string());
                let _ = tx.send(AssistantMessageEvent::Error {
                    reason: StopReason::Aborted,
                    error: output.clone(),
                });
                return Ok(());
            }
        };
        let Some(sse) = next else {
            break;
        };
        let sse = sse?;

        let data = &sse.data;
        if data.is_empty() {
            continue;
        }

        // Parse with the shared repair helper (match TS `parseJsonWithRepair`:
        // tolerate partial/repairable JSON).
        let event: AnthropicSseEvent =
            match crate::utils::json_parse::parse_json_with_repair(data) {
                Ok(event) => event,
                Err(_) => continue,
            };

        match event {
            AnthropicSseEvent::MessageStart { message } => {
                output.response_id = Some(message.id);
                // Only report a response model when the provider answered with a
                // different one (TS `if (responseModel !== model.id)`).
                if message.model != model.id {
                    output.response_model = Some(message.model.clone());
                }
                // A fallback model answers at its own price (TS `usageModel`).
                if message.model != model.id {
                    if let Some(fallback) = allowed_fallback_models.iter().find(|fallback| {
                        fallback.provider == model.provider && fallback.model == message.model
                    }) {
                        usage_model_cost = Some(fallback.cost.clone());
                    }
                }
                output.usage.input = message.usage.input_tokens.unwrap_or(0);
                output.usage.output = message.usage.output_tokens.unwrap_or(0);
                output.usage.cache_read = message.usage.cache_read_input_tokens.unwrap_or(0);
                output.usage.cache_write = message.usage.cache_creation_input_tokens.unwrap_or(0);
                output.usage.cache_write_1h = message
                    .usage
                    .cache_creation
                    .as_ref()
                    .and_then(|c| c.ephemeral_1h_input_tokens);
                output.usage.total_tokens = output.usage.input
                    + output.usage.output
                    + output.usage.cache_read
                    + output.usage.cache_write;
            }
            AnthropicSseEvent::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                AnthropicContentBlockStart::Text { .. } => {
                    let block = ContentBlock::text("");
                    let content_idx = blocks.len();
                    output.content.push(block.clone());
                    blocks.push(BlockInfo {
                        block,
                        index,
                        partial_json: String::new(),
                    });
                    let _ = tx.send(AssistantMessageEvent::TextStart {
                        content_index: content_idx,
                        partial: output.clone(),
                    });
                }
                AnthropicContentBlockStart::Thinking { .. } => {
                    let block = ContentBlock::Thinking {
                        thinking: String::new(),
                        thinking_signature: None,
                        redacted: None,
                    };
                    let content_idx = blocks.len();
                    output.content.push(block.clone());
                    blocks.push(BlockInfo {
                        block,
                        index,
                        partial_json: String::new(),
                    });
                    let _ = tx.send(AssistantMessageEvent::ThinkingStart {
                        content_index: content_idx,
                        partial: output.clone(),
                    });
                }
                AnthropicContentBlockStart::RedactedThinking { data } => {
                    let block = ContentBlock::Thinking {
                        thinking: "[Reasoning redacted]".to_string(),
                        thinking_signature: Some(data),
                        redacted: Some(true),
                    };
                    let content_idx = blocks.len();
                    output.content.push(block.clone());
                    blocks.push(BlockInfo {
                        block,
                        index,
                        partial_json: String::new(),
                    });
                    let _ = tx.send(AssistantMessageEvent::ThinkingStart {
                        content_index: content_idx,
                        partial: output.clone(),
                    });
                }
                AnthropicContentBlockStart::ToolUse {
                    id, name, input, ..
                } => {
                    let block = ContentBlock::ToolCall {
                        id,
                        name,
                        arguments: input,
                        thought_signature: None,
                        namespace: None,
                    };
                    let content_idx = blocks.len();
                    output.content.push(block.clone());
                    blocks.push(BlockInfo {
                        block,
                        index,
                        partial_json: String::new(),
                    });
                    let _ = tx.send(AssistantMessageEvent::ToolCallStart {
                        content_index: content_idx,
                        partial: output.clone(),
                    });
                }
            },
            AnthropicSseEvent::ContentBlockDelta { index, delta } => {
                let block_info = blocks.iter_mut().find(|b| b.index == index);
                if let Some(bi) = block_info {
                    match delta {
                        AnthropicContentDelta::TextDelta { text } => {
                            if let ContentBlock::Text {
                                text: ref mut t, ..
                            } = bi.block
                            {
                                t.push_str(&text);
                            }
                            output.content[bi.index] = bi.block.clone();
                            let _ = tx.send(AssistantMessageEvent::TextDelta {
                                content_index: bi.index,
                                delta: text,
                                partial: output.clone(),
                            });
                        }
                        AnthropicContentDelta::ThinkingDelta { thinking } => {
                            if let ContentBlock::Thinking {
                                thinking: ref mut t,
                                ..
                            } = bi.block
                            {
                                t.push_str(&thinking);
                            }
                            output.content[bi.index] = bi.block.clone();
                            let _ = tx.send(AssistantMessageEvent::ThinkingDelta {
                                content_index: bi.index,
                                delta: thinking,
                                partial: output.clone(),
                            });
                        }
                        AnthropicContentDelta::SignatureDelta { signature } => {
                            if let ContentBlock::Thinking {
                                thinking_signature: ref mut sig,
                                ..
                            } = bi.block
                            {
                                *sig = Some(sig.as_deref().unwrap_or("").to_string() + &signature);
                            }
                        }
                        AnthropicContentDelta::InputJsonDelta { partial_json } => {
                            bi.partial_json.push_str(&partial_json);
                            if let ContentBlock::ToolCall {
                                arguments: ref mut args,
                                ..
                            } = bi.block
                            {
                                // Try to parse the partial JSON
                                if let Ok(parsed) = serde_json::from_str::<Value>(&bi.partial_json)
                                {
                                    *args = parsed;
                                }
                            }
                            output.content[bi.index] = bi.block.clone();
                            let _ = tx.send(AssistantMessageEvent::ToolCallDelta {
                                content_index: bi.index,
                                delta: partial_json,
                                partial: output.clone(),
                            });
                        }
                    }
                }
            }
            AnthropicSseEvent::ContentBlockStop { index } => {
                let block_info = blocks.iter().find(|b| b.index == index);
                if let Some(bi) = block_info {
                    match &bi.block {
                        ContentBlock::Text { text, .. } => {
                            let _ = tx.send(AssistantMessageEvent::TextEnd {
                                content_index: bi.index,
                                content: text.clone(),
                                partial: output.clone(),
                            });
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            let _ = tx.send(AssistantMessageEvent::ThinkingEnd {
                                content_index: bi.index,
                                content: thinking.clone(),
                                partial: output.clone(),
                            });
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            let tool_call = crate::types::ToolCall::new(
                                id.clone(),
                                name.clone(),
                                arguments.clone(),
                            );
                            let _ = tx.send(AssistantMessageEvent::ToolCallEnd {
                                content_index: bi.index,
                                tool_call,
                                partial: output.clone(),
                            });
                        }
                        ContentBlock::Image { .. } => {}
                    }
                }
            }
            AnthropicSseEvent::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.stop_reason {
                    let (stop_reason, error_message) =
                        map_stop_reason(&reason, delta.stop_details.as_ref());
                    output.stop_reason = stop_reason;
                    output.raw_stop_reason = Some(reason.clone());
                    if let Some(msg) = error_message {
                        output.error_message = Some(msg);
                    }
                }
                if let Some(input) = usage.input_tokens {
                    output.usage.input = input;
                }
                if let Some(output_tokens) = usage.output_tokens {
                    output.usage.output = output_tokens;
                }
                if let Some(cache_read) = usage.cache_read_input_tokens {
                    output.usage.cache_read = cache_read;
                }
                if let Some(cache_write) = usage.cache_creation_input_tokens {
                    output.usage.cache_write = cache_write;
                }
                if let Some(tokens) = usage
                    .output_tokens_details
                    .as_ref()
                    .and_then(|d| d.thinking_tokens)
                {
                    output.usage.reasoning = Some(tokens);
                }
                output.usage.total_tokens = output.usage.input
                    + output.usage.output
                    + output.usage.cache_read
                    + output.usage.cache_write;
            }
            AnthropicSseEvent::MessageStop => {
                // Stream ended normally
            }
        }
    }

    // Calculate cost; a fallback answer is priced with the fallback's rates.
    match usage_model_cost {
        Some(cost) => {
            let mut fallback_model = model.clone();
            fallback_model.cost = cost;
            if let Some(response_model) = output.response_model.clone() {
                fallback_model.id = response_model;
            }
            crate::models::calculate_cost(&fallback_model, &mut output.usage);
        }
        None => crate::models::calculate_cost(model, &mut output.usage),
    }

    // If the stream ended normally but an abort was requested (e.g. the
    // provider closed the connection right as the user cancelled), mark the
    // message as aborted — matching TS `signal.aborted ? "aborted" : "error"`.
    if let Some(ref rx) = signal {
        if *rx.borrow() {
            output.stop_reason = StopReason::Aborted;
            output.error_message = Some("Request was aborted".to_string());
            let _ = tx.send(AssistantMessageEvent::Error {
                reason: StopReason::Aborted,
                error: output.clone(),
            });
            return Ok(());
        }
    }

    let _ = tx.send(AssistantMessageEvent::Done {
        reason: output.stop_reason.clone(),
        message: output,
    });

    Ok(())
}

// ============================================================================
// streamSimpleAnthropic
// ============================================================================

/// Stream a completion from Anthropic with simplified options.
#[must_use]
pub fn stream_simple_anthropic(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    // Route through the shared helper (match TS `buildBaseOptions`) so
    // hooks, `http_client`, `sampling_params`, `tool_choice`, `service_tier`,
    // `reasoning_effort` and `thinking_budgets` are preserved.
    let full_opts = crate::providers::simple_options::build_base_options(model, context, options, None);
    stream_anthropic(model, context, Some(&full_opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::types::ModelCost;

    fn test_model() -> Model {
        Model {
            prompt_cache: None,
            id: "m".into(),
            name: "m".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 4096,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    /// Native tool changes emit `tool_removal`/`tool_addition` blocks in the
    /// pending system message (TS native tool changes).
    #[test]
    fn native_tool_changes_emit_tool_blocks() {
        let model = test_model();
        let user = Message::User {
            content: vec![ContentBlock::text("hi")],
            timestamp: 0,
        };
        let assistant = Message::Assistant {
            content: vec![ContentBlock::text("ok")],
            api: "a".into(),
            provider: "p".into(),
            model: "m".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 2,
        };
        let system = Message::System {
            content: crate::types::SystemContent::Text(String::new()),
            sections: None,
            tools_added: Some(vec![crate::types::Tool {
                name: "b".into(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
                constrained_sampling: None,
            }]),
            tools_removed: Some(vec![crate::types::ToolReference { name: "a".into() }]),
            timestamp: 1,
        };
        let out = convert_messages_full(
            &[user, system, assistant],
            &model,
            None,
            true,
            true,
        );
        let vals = serde_json::to_value(&out).unwrap();
        assert_eq!(vals[1]["role"], "system");
        assert_eq!(vals[1]["content"][0]["type"], "tool_removal");
        assert_eq!(vals[1]["content"][0]["tool"]["tool_name"], "a");
        assert_eq!(vals[1]["content"][1]["type"], "tool_addition");
        assert_eq!(vals[1]["content"][1]["tool"]["tool_name"], "b");

        // Without native tool changes the same system message only carries text
        // (empty here → no system message).
        let plain = convert_messages_full(
            &[
                Message::User {
                    content: vec![ContentBlock::text("hi")],
                    timestamp: 0,
                },
                Message::System {
                    content: crate::types::SystemContent::Text(String::new()),
                    sections: None,
                    tools_added: Some(vec![crate::types::Tool {
                        name: "b".into(),
                        description: String::new(),
                        parameters: serde_json::json!({"type": "object"}),
                        constrained_sampling: None,
                    }]),
                    tools_removed: None,
                    timestamp: 1,
                },
            ],
            &model,
            None,
            true,
            false,
        );
        let roles: Vec<&str> = plain.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user"]);
    }

    /// P1-7 native mid-convo: later system messages flush right before the next
    /// assistant (or at the end); without native support they are dropped
    /// (the transcript was collapsed into the leading prompt).
    #[test]
    fn native_system_messages_flush_before_assistant() {
        let model = test_model();
        let user = Message::User {
            content: vec![ContentBlock::text("hi")],
            timestamp: 0,
        };
        let system = Message::System {
            content: crate::types::SystemContent::Text("update".into()),
            sections: None,
            tools_added: None,
            tools_removed: None,
            timestamp: 1,
        };
        let assistant = Message::Assistant {
            content: vec![ContentBlock::text("ok")],
            api: "a".into(),
            provider: "p".into(),
            model: "m".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 2,
        };

        let native = convert_messages(
            &[user.clone(), system.clone(), assistant.clone()],
            &model,
            None,
            true,
        );
        let roles: Vec<&str> = native.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "system", "assistant"]);

        let collapsed = convert_messages(
            &[user.clone(), system.clone(), assistant.clone()],
            &model,
            None,
            false,
        );
        let roles: Vec<&str> = collapsed.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant"]);

        let trailing = convert_messages(
            &[user, assistant, system],
            &model,
            None,
            true,
        );
        let roles: Vec<&str> = trailing.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "system"]);
    }

    /// TS `anthropicApiKeyAuth().resolve`: an explicit/stored key beats every env var.
    #[test]
    fn test_resolve_anthropic_auth_explicit_key_wins() {
        let auth = resolve_anthropic_auth(
            Some("stored".into()),
            Some("bearer".into()),
            Some("oauth".into()),
            Some("env".into()),
        );
        assert_eq!(
            auth,
            AnthropicAuth {
                api_key: Some("stored".into()),
                use_bearer: false,
                auth_token: None,
            }
        );
    }

    /// `ANTHROPIC_AUTH_TOKEN` outranks the OAuth/API-key env vars and is sent
    /// as a Bearer credential (TS #5871/#6148).
    #[test]
    fn test_resolve_anthropic_auth_auth_token_becomes_bearer() {
        let auth = resolve_anthropic_auth(
            None,
            Some("bearer".into()),
            Some("oauth".into()),
            Some("env".into()),
        );
        assert!(auth.use_bearer);
        assert_eq!(auth.auth_token.as_deref(), Some("bearer"));
        assert_eq!(auth.api_key, None);
    }

    /// `ANTHROPIC_OAUTH_TOKEN` outranks `ANTHROPIC_API_KEY`.
    #[test]
    fn test_resolve_anthropic_auth_oauth_token_beats_api_key() {
        let auth = resolve_anthropic_auth(
            None,
            None,
            Some("oauth".into()),
            Some("env".into()),
        );
        assert_eq!(auth.api_key.as_deref(), Some("oauth"));
        assert!(!auth.use_bearer);
    }

    #[test]
    fn test_resolve_anthropic_auth_api_key_fallback() {
        let auth = resolve_anthropic_auth(None, None, None, Some("env".into()));
        assert_eq!(auth.api_key.as_deref(), Some("env"));
        assert!(!auth.use_bearer);
    }

    /// A blank explicit key counts as unset and falls through to env.
    #[test]
    fn test_resolve_anthropic_auth_empty_explicit_falls_through() {
        let auth = resolve_anthropic_auth(
            Some(String::new()),
            None,
            None,
            Some("env".into()),
        );
        assert_eq!(auth.api_key.as_deref(), Some("env"));
    }

    /// Non-anthropic providers (minimax, fireworks, …) must use their own env
    /// key and ignore `ANTHROPIC_*` variables, even though they speak the
    /// Anthropic wire format.
    #[test]
    fn test_resolve_anthropic_provider_auth_non_anthropic_uses_provider_env() {
        let auth = resolve_anthropic_provider_auth(
            "minimax",
            None,
            Some("anthropic-bearer".into()),
            Some("anthropic-oauth".into()),
            Some("anthropic-key".into()),
            Some("minimax-key".into()),
        );
        assert_eq!(auth.api_key.as_deref(), Some("minimax-key"));
        assert!(!auth.use_bearer);
    }

    /// `github-copilot` uses its own env key (sent as Bearer by the header builder).
    #[test]
    fn test_resolve_anthropic_provider_auth_copilot_uses_provider_env() {
        let auth = resolve_anthropic_provider_auth(
            "github-copilot",
            None,
            None,
            None,
            Some("anthropic-key".into()),
            Some("copilot-token".into()),
        );
        assert_eq!(auth.api_key.as_deref(), Some("copilot-token"));
        assert!(!auth.use_bearer);
    }

    /// An explicit key still wins for non-anthropic providers.
    #[test]
    fn test_resolve_anthropic_provider_auth_explicit_wins() {
        let auth = resolve_anthropic_provider_auth(
            "minimax",
            Some("stored".into()),
            None,
            None,
            None,
            Some("minimax-key".into()),
        );
        assert_eq!(auth.api_key.as_deref(), Some("stored"));
    }

    /// A plain API key is sent as `x-api-key`; an `sk-ant-oat…` OAuth token is
    /// upgraded to a Bearer credential with Claude Code identity headers.
    #[test]
    fn test_anthropic_auth_headers_api_key_vs_oauth_token() {
        let api_key =
            anthropic_auth_headers("anthropic", Some("sk-ant-api03-key"), None, false);
        assert_eq!(
            api_key,
            vec![("x-api-key".to_string(), "sk-ant-api03-key".to_string())]
        );

        let oauth = anthropic_auth_headers("anthropic", Some("sk-ant-oat01-token"), None, false);
        assert_eq!(
            oauth,
            vec![
                (
                    "authorization".to_string(),
                    "Bearer sk-ant-oat01-token".to_string()
                ),
                (
                    "user-agent".to_string(),
                    format!("claude-cli/{CLAUDE_CODE_VERSION}")
                ),
                ("x-app".to_string(), "cli".to_string()),
            ]
        );
    }

    /// A gateway bearer token never also emits `x-api-key`.
    #[test]
    fn test_anthropic_auth_headers_bearer() {
        let headers =
            anthropic_auth_headers("anthropic", Some("ignored"), Some("gateway"), true);
        assert_eq!(
            headers,
            vec![("authorization".to_string(), "Bearer gateway".to_string())]
        );
    }

    /// GitHub Copilot always authenticates with Bearer (even for a token that
    /// is not an `sk-ant-oat…` OAuth token and without Claude Code headers).
    #[test]
    fn test_anthropic_auth_headers_github_copilot_uses_bearer() {
        let headers = anthropic_auth_headers(
            "github-copilot",
            Some("copilot-token"),
            None,
            false,
        );
        assert_eq!(
            headers,
            vec![("authorization".to_string(), "Bearer copilot-token".to_string())]
        );
    }

    fn make_test_model() -> Model {
        Model {
            prompt_cache: None,
            id: "claude-sonnet-4-6".into(),
            name: "Claude Sonnet 4.6".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com/v1/messages".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec!["text".into(), "image".into()],
            cost: ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 6.0,
                tiers: vec![],
            },
            context_window: 200_000,
            max_tokens: 8192,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    // ============================================================
    // normalize_tool_call_id tests
    // ============================================================

    #[test]
    fn test_normalize_tool_call_id_alphanumeric() {
        assert_eq!(normalize_tool_call_id("abc123"), "abc123");
    }

    #[test]
    fn test_normalize_tool_call_id_with_special_chars() {
        assert_eq!(normalize_tool_call_id("tool_use:123!"), "tool_use_123_");
    }

    #[test]
    fn test_normalize_tool_call_id_truncation() {
        let long_id = "a".repeat(100);
        assert_eq!(normalize_tool_call_id(&long_id).len(), 64);
    }

    // ============================================================
    // map_stop_reason tests
    // ============================================================

    #[test]
    fn test_map_stop_reason_end_turn() {
        assert_eq!(map_stop_reason("end_turn", None), (StopReason::Stop, None));
    }

    #[test]
    fn test_map_stop_reason_max_tokens() {
        assert_eq!(
            map_stop_reason("max_tokens", None),
            (StopReason::Length, None)
        );
    }

    #[test]
    fn test_map_stop_reason_tool_use() {
        assert_eq!(
            map_stop_reason("tool_use", None),
            (StopReason::ToolUse, None)
        );
    }

    #[test]
    fn test_map_stop_reason_stop_sequence() {
        assert_eq!(
            map_stop_reason("stop_sequence", None),
            (StopReason::Stop, None)
        );
    }

    #[test]
    fn test_map_stop_reason_refusal_with_explanation() {
        let details = AnthropicStopDetails {
            explanation: Some("I cannot help with that".to_string()),
        };
        assert_eq!(
            map_stop_reason("refusal", Some(&details)),
            (
                StopReason::Error,
                Some("I cannot help with that".to_string())
            )
        );
    }

    #[test]
    fn test_map_stop_reason_refusal_without_explanation() {
        assert_eq!(
            map_stop_reason("refusal", None),
            (
                StopReason::Error,
                Some("The model refused to complete the request".to_string())
            )
        );
    }

    #[test]
    fn test_map_stop_reason_sensitive() {
        assert_eq!(
            map_stop_reason("sensitive", None),
            (
                StopReason::Error,
                Some("Provider stopped with: sensitive".to_string())
            )
        );
    }

    #[test]
    fn test_map_stop_reason_unknown() {
        // Unknown stop reasons must surface as an error stop reason (matching
        // TS throw → catch → error), not panic the stream task.
        let (reason, msg) = map_stop_reason("unknown_reason", None);
        assert_eq!(reason, StopReason::Error);
        assert_eq!(msg.as_deref(), Some("Unhandled stop reason: unknown_reason"));
    }

    // ============================================================
    // convert_messages tests
    // ============================================================

    #[test]
    fn test_convert_messages_user_only() {
        let model = make_test_model();
        let messages = vec![Message::User {
            content: vec![ContentBlock::text("Hello")],
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages, &model, None, false);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "user");
    }

    #[test]
    fn test_convert_messages_assistant_with_text() {
        let model = make_test_model();
        let messages = vec![Message::Assistant {
            content: vec![ContentBlock::text("Hi!")],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages, &model, None, false);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "assistant");
    }

    #[test]
    fn test_convert_messages_tool_result() {
        let model = make_test_model();
        let messages = vec![Message::ToolResult {
            tool_call_id: "toolu_001".into(),
            tool_name: "read".into(),
            content: vec![ContentBlock::text("file contents")],
            details: None,
            is_error: false,
            usage: None,
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages, &model, None, false);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "user");
    }

    #[test]
    fn test_convert_messages_assistant_with_tool_calls() {
        let model = make_test_model();
        let messages = vec![
            Message::User {
                content: vec![ContentBlock::text("What's the weather?")],
                timestamp: 1000,
            },
            Message::Assistant {
                content: vec![
                    ContentBlock::text("Let me check..."),
                    ContentBlock::ToolCall {
                        id: "tool_1".into(),
                        name: "get_weather".into(),
                        arguments: serde_json::json!({"city": "NYC"}),
                        thought_signature: None,
                        namespace: None,
                    },
                ],
                api: "anthropic-messages".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 1000,
            },
            Message::ToolResult {
                tool_call_id: "tool_1".into(),
                tool_name: "get_weather".into(),
                content: vec![ContentBlock::text("72F sunny")],
                details: None,
                is_error: false,
                usage: None,
                timestamp: 1000,
            },
        ];
        let converted = convert_messages(&messages, &model, None, false);
        assert_eq!(converted.len(), 3);
        assert_eq!(converted[0].role, "user");
        assert_eq!(converted[1].role, "assistant");
        assert_eq!(converted[2].role, "user");
    }

    #[test]
    fn test_convert_messages_thinking_is_skipped() {
        let model = make_test_model();
        let messages = vec![Message::Assistant {
            content: vec![
                ContentBlock::Thinking {
                    thinking: "Let me think...".into(),
                    thinking_signature: None,
                    redacted: None,
                },
                ContentBlock::text("The answer is 42."),
            ],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages, &model, None, false);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "assistant");
        // Thinking blocks should be filtered out from the API request
    }

    #[test]
    fn test_convert_messages_empty_user_content() {
        let model = make_test_model();
        let messages = vec![Message::User {
            content: vec![ContentBlock::text("")],
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages, &model, None, false);
        // Empty user messages should be skipped
        assert_eq!(converted.len(), 0);
    }

    #[test]
    fn test_convert_messages_tool_call_id_normalized() {
        let model = make_test_model();
        let messages = vec![Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "tool_use:123!@#".into(),
                name: "test".into(),
                arguments: serde_json::json!({}),
                thought_signature: None,
                namespace: None,
            }],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages, &model, None, false);
        assert_eq!(converted.len(), 1);
        if let AnthropicContent::Blocks(blocks) = &converted[0].content {
            if let AnthropicContentBlock::ToolUse { id, .. } = &blocks[0] {
                assert!(!id.contains('!'));
                assert!(!id.contains('@'));
                assert!(!id.contains('#'));
            } else {
                panic!("expected ToolUse");
            }
        } else {
            panic!("expected Blocks");
        }
    }

    // ============================================================
    // convert_tools tests
    // ============================================================

    #[test]
    fn test_convert_tools_defer_loading() {
        let tools = vec![Tool {
            name: "deferred_tool".into(),
            description: "d".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            constrained_sampling: None,
        }];
        let converted = convert_tools(&tools, None, true, false).unwrap();
        assert_eq!(converted[0].defer_loading, Some(true));
        let converted = convert_tools(&tools, None, false, false).unwrap();
        assert_eq!(converted[0].defer_loading, None);
    }

    #[test]
    fn test_convert_messages_tool_result_carries_content_only() {
        // Tool references were removed with the request-level deferred split
        // (#9548): a tool result converts to a plain tool_result block, and any
        // added tool names do not produce tool_reference blocks.
        let messages = vec![Message::ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "bash".into(),
            content: vec![ContentBlock::text("output text")],
            details: None,
            is_error: false,
            usage: None,
            timestamp: 0,
        }];
        let converted = convert_messages(&messages, &model_for_tests(), None, false);
        assert_eq!(converted.len(), 1);
        let AnthropicContent::Blocks(blocks) = &converted[0].content else {
            panic!("expected blocks");
        };
        assert_eq!(blocks.len(), 1, "no sibling tool_reference blocks");
        match &blocks[0] {
            AnthropicContentBlock::ToolResult { content, .. } => {
                assert!(
                    matches!(content, AnthropicToolResultContent::String(text) if text == "output text"),
                    "the result's own content is passed through"
                );
            }
            other => panic!("expected a tool_result block, got {other:?}"),
        }
    }

    fn model_for_tests() -> Model {
        Model {
            prompt_cache: None,
            id: "claude-sonnet-4-5".into(),
            name: "x".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec!["text".into(), "image".into()],
            cost: ModelCost::default(),
            context_window: 200000,
            max_tokens: 32000,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn test_convert_tools_empty() {
        let converted = convert_tools(&[], None, false, false).unwrap();
        assert!(converted.is_empty());
    }

    #[test]
    fn test_convert_tools_single() {
        let tools = vec![Tool {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            constrained_sampling: None,
        }];
        let converted = convert_tools(&tools, None, false, false).unwrap();
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].name, "read");
        assert_eq!(converted[0].description, "Read a file");
    }

    #[test]
    fn test_convert_tools_cache_control_on_last_tool() {
        // cache_control only lands on the last tool definition (match TS).
        let tools = vec![
            Tool {
                name: "read".into(),
                description: "Read a file".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                constrained_sampling: None,
            },
            Tool {
                name: "grep".into(),
                description: "Search".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                constrained_sampling: None,
            },
        ];
        let cc = AnthropicCacheControl {
            cache_type: "ephemeral".to_string(),
            ttl: None,
        };
        let converted = convert_tools(&tools, Some(&cc), false, false).unwrap();
        assert!(converted[0].cache_control.is_none());
        assert!(converted[1].cache_control.is_some());
    }

    #[test]
    fn test_convert_messages_cache_control_on_last_user_message() {
        // Last user message string content becomes a text block carrying
        // cache_control (match TS convertMessages).
        let model = make_test_model();
        let messages = vec![
            Message::User {
                content: vec![ContentBlock::text("First")],
                timestamp: 0,
            },
            Message::User {
                content: vec![ContentBlock::text("Second")],
                timestamp: 0,
            },
        ];
        let cc = AnthropicCacheControl {
            cache_type: "ephemeral".to_string(),
            ttl: Some("1h".to_string()),
        };
        let converted = convert_messages(&messages, &model, Some(&cc), false);
        assert_eq!(converted.len(), 2);
        match &converted[1].content {
            AnthropicContent::Blocks(blocks) => match &blocks[0] {
                AnthropicContentBlock::Text {
                    text,
                    cache_control,
                } => {
                    assert_eq!(text, "Second");
                    assert!(cache_control.is_some());
                }
                _ => panic!("expected text block"),
            },
            _ => panic!("expected blocks"),
        }
        // Without cache control the plain string form is preserved.
        let converted = convert_messages(&messages, &model, None, false);
        assert!(matches!(converted[1].content, AnthropicContent::String(_)));
    }

    /// Mid-conversation models without native tool changes keep the transcript
    /// system messages, so the request-level tool list must be replayed from
    /// the transcript rather than read from `Context.tools`
    /// (TS `getCurrentTools(context.messages)`).
    #[tokio::test]
    async fn transcript_tools_are_declared_without_native_tool_changes() {
        let model = Model {
            compat: Some(crate::types::ModelCompat::AnthropicMessages(
                crate::types::AnthropicMessagesCompat {
                    supports_mid_convo_system_messages: Some(true),
                    ..Default::default()
                },
            )),
            ..test_model()
        };
        let context = Context {
            system_prompt: None,
            tools: None,
            messages: vec![
                Message::System {
                    content: crate::types::SystemContent::Text("sys".into()),
                    sections: None,
                    tools_added: Some(vec![crate::types::Tool {
                        name: "read".into(),
                        description: String::new(),
                        parameters: serde_json::json!({"type": "object"}),
                        constrained_sampling: None,
                    }]),
                    tools_removed: None,
                    timestamp: 0,
                },
                Message::User {
                    content: vec![ContentBlock::text("hi")],
                    timestamp: 1,
                },
            ],
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = captured.clone();
        let options = StreamOptions {
            api_key: Some("test-key".into()),
            // Cancels the request after recording the payload.
            on_payload: Some(std::sync::Arc::new(
                move |payload: serde_json::Value| {
                    let sink = sink.clone();
                    Box::pin(async move {
                        *sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some(payload);
                        None
                    })
                },
            )),
            ..Default::default()
        };

        let _ = stream_anthropic(&model, &context, Some(&options)).result().await;

        let body = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| panic!("request payload must be captured"));
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools must be declared: {body}"))
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, vec!["read"]);
    }

    /// A model with server-side fallbacks advertises them plus the matching
    /// beta header (TS `params.fallbacks` / `SERVER_SIDE_FALLBACK_BETA`).
    #[tokio::test]
    async fn server_side_fallbacks_are_advertised() {
        let captured: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = captured.clone();
        let model = Model {
            compat: Some(crate::types::ModelCompat::AnthropicMessages(
                crate::types::AnthropicMessagesCompat {
                    allowed_fallback_models: Some(vec![crate::types::AnthropicAllowedFallbackModel {
                        provider: "anthropic".to_string(),
                        model: "claude-opus-4-8".to_string(),
                        cost: crate::types::ModelCost {
                            input: 5.0,
                            output: 25.0,
                            cache_read: 0.5,
                            cache_write: 6.25,
                            tiers: vec![],
                        },
                    }]),
                    ..Default::default()
                },
            )),
            ..test_model()
        };
        let context = Context {
            system_prompt: None,
            tools: None,
            messages: vec![Message::User {
                content: vec![ContentBlock::text("hi")],
                timestamp: 0,
            }],
        };
        let options = StreamOptions {
            api_key: Some("test-key".into()),
            on_payload: Some(std::sync::Arc::new(move |payload: serde_json::Value| {
                let sink = sink.clone();
                Box::pin(async move {
                    *sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(payload);
                    None
                })
            })),
            ..Default::default()
        };

        let _ = stream_anthropic(&model, &context, Some(&options)).result().await;

        let body = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| panic!("request payload must be captured"));
        assert_eq!(
            body["fallbacks"],
            serde_json::json!([{"model": "claude-opus-4-8"}]),
            "fallbacks carry model ids only: {body}"
        );
    }

    /// Without fallbacks neither the parameter nor the beta is sent.
    #[tokio::test]
    async fn no_fallbacks_means_no_parameter() {
        let captured: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = captured.clone();
        let context = Context {
            system_prompt: None,
            tools: None,
            messages: vec![Message::User {
                content: vec![ContentBlock::text("hi")],
                timestamp: 0,
            }],
        };
        let options = StreamOptions {
            api_key: Some("test-key".into()),
            on_payload: Some(std::sync::Arc::new(move |payload: serde_json::Value| {
                let sink = sink.clone();
                Box::pin(async move {
                    *sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(payload);
                    None
                })
            })),
            ..Default::default()
        };

        let _ = stream_anthropic(&test_model(), &context, Some(&options)).result().await;

        let body = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| panic!("request payload must be captured"));
        assert!(body.get("fallbacks").is_none(), "{body}");
    }

}

#[cfg(test)]
mod abort_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use futures::StreamExt;

    fn abort_test_model(addr: &str) -> Model {
        Model {
            prompt_cache: None,
            id: "claude-test".to_string(),
            name: "Claude Test".to_string(),
            api: "anthropic".to_string(),
            provider: "anthropic".to_string(),
            base_url: format!("http://{addr}"),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: crate::types::ModelCost::default(),
            context_window: 200_000,
            max_tokens: 4096,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    /// Aborting mid-stream must interrupt the idle SSE read immediately and
    /// mark the message `StopReason::Aborted` (matching TS active abort).
    #[tokio::test]
    async fn abort_interrupts_idle_sse_stream_and_marks_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
            // One text block, then hold the connection open (idle stream).
            let body = concat!(
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{}",
                body
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, resp.as_bytes()).await;
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });

        let model = abort_test_model(&addr.to_string());
        let context = Context {
            system_prompt: Some("You are helpful".into()),
            messages: vec![],
            tools: None,
        };
        let (tx, rx) = tokio::sync::watch::channel(false);
        let opts = StreamOptions {
            api_key: Some("test-key".into()),
            signal: Some(rx),
            ..Default::default()
        };
        let mut stream = stream_anthropic(&model, &context, Some(&opts));

        let mut saw_delta = false;
        for _ in 0..100 {
            match tokio::time::timeout(std::time::Duration::from_millis(100), stream.next()).await {
                Ok(Some(AssistantMessageEvent::TextDelta { .. })) => {
                    saw_delta = true;
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("stream ended before abort"),
                Err(_) => {}
            }
        }
        assert!(saw_delta, "must receive the text delta first");

        tx.send(true).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap_or_else(|_| panic!("abort must interrupt the idle stream within 5s"))
            .unwrap_or_else(|| panic!("stream must yield an event after abort"));
        match result {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, StopReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => panic!("expected Error(Aborted), got {other:?}"),
        }
    }

}
