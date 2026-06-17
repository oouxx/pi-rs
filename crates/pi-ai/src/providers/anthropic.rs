//! Anthropic Messages API provider.
//!
//! Thin wrapper around the Anthropic Messages API using reqwest for HTTP
//! and SSE streaming. Converts between pi-ai types and Anthropic API format.
//!
//! Ported from `packages/ai/src/providers/anthropic.ts`.

use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{
    AnthropicMessagesCompat, AssistantMessage, AssistantMessageEvent, CacheRetention, ContentBlock,
    Context, Message, Model, ModelCompat, SimpleStreamOptions, StopReason, StreamOptions, Tool,
    Usage,
};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::sse::parse_sse_body;

// ============================================================================
// Constants
// ============================================================================

const ANTHROPIC_VERSION: &str = "2023-06-01";
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
#[allow(dead_code)]
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

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

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(rename = "tool_use_id")]
        tool_use_id: String,
        content: String,
        #[serde(rename = "is_error", skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: String,
    #[serde(rename = "media_type")]
    media_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
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
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
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
            supports_eager_tool_input_streaming: None,
            supports_long_cache_retention: None,
            send_session_affinity_headers: None,
            supports_cache_control_on_tools: None,
            allow_empty_signature: None,
            force_adaptive_thinking: None,
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

/// Convert pi-ai messages to Anthropic API message params.
pub(crate) fn convert_messages(messages: &[Message], _model: &Model) -> Vec<AnthropicMessageParam> {
    let mut params: Vec<AnthropicMessageParam> = Vec::new();

    for msg in messages {
        match msg {
            Message::User { content, .. } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                if !text.trim().is_empty() {
                    params.push(AnthropicMessageParam {
                        role: "user".to_string(),
                        content: AnthropicContent::String(text),
                    });
                }

                // Handle images
                let images: Vec<_> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Image { data, mime_type } => {
                            Some(AnthropicContentBlock::Image {
                                source: AnthropicImageSource {
                                    source_type: "base64".to_string(),
                                    media_type: mime_type.clone(),
                                    data: data.clone(),
                                },
                            })
                        }
                        _ => None,
                    })
                    .collect();

                if !images.is_empty() {
                    // For image messages, we need to use content blocks
                    // If there are text blocks, include them alongside images
                    let blocks: Vec<AnthropicContentBlock> = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => {
                                Some(AnthropicContentBlock::Text { text: text.clone() })
                            }
                            ContentBlock::Image { data, mime_type } => {
                                Some(AnthropicContentBlock::Image {
                                    source: AnthropicImageSource {
                                        source_type: "base64".to_string(),
                                        media_type: mime_type.clone(),
                                        data: data.clone(),
                                    },
                                })
                            }
                            _ => None,
                        })
                        .collect();

                    // Replace the last user message with blocks version
                    if let Some(last) = params.last_mut() {
                        if last.role == "user" {
                            last.content = AnthropicContent::Blocks(blocks);
                        }
                    }
                }
            }
            Message::Assistant { content, .. } => {
                let blocks: Vec<AnthropicContentBlock> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => {
                            Some(AnthropicContentBlock::Text { text: text.clone() })
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => Some(AnthropicContentBlock::ToolUse {
                            id: normalize_tool_call_id(id),
                            name: name.clone(),
                            input: arguments.clone(),
                        }),
                        ContentBlock::Thinking { .. } => {
                            // Thinking blocks are not sent back to the API
                            None
                        }
                        _ => None,
                    })
                    .collect();

                if !blocks.is_empty() {
                    params.push(AnthropicMessageParam {
                        role: "assistant".to_string(),
                        content: AnthropicContent::Blocks(blocks),
                    });
                }
            }
            Message::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                params.push(AnthropicMessageParam {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(vec![AnthropicContentBlock::ToolResult {
                        tool_use_id: normalize_tool_call_id(tool_call_id),
                        content: text,
                        is_error: if *is_error { Some(true) } else { None },
                    }]),
                });
            }
        }
    }

    params
}

// ============================================================================
// Tool conversion
// ============================================================================

/// Convert pi-ai tools to Anthropic API tool definitions.
pub(crate) fn convert_tools(tools: &[Tool]) -> Vec<AnthropicTool> {
    tools
        .iter()
        .map(|t| AnthropicTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.parameters.clone(),
        })
        .collect()
}

// ============================================================================
// Stop reason mapping
// ============================================================================

/// Map Anthropic stop reason to pi-ai StopReason.
pub fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::Stop,
        "max_tokens" => StopReason::Length,
        "tool_use" => StopReason::ToolUse,
        "stop_sequence" => StopReason::Stop,
        _ => StopReason::Stop,
    }
}

// ============================================================================
// Cache control resolution
// ============================================================================

#[allow(dead_code)]
fn resolve_cache_retention(retention: Option<&CacheRetention>) -> CacheRetention {
    match retention {
        Some(r) => r.clone(),
        None => {
            if std::env::var("PI_CACHE_RETENTION").as_deref() == Ok("long") {
                CacheRetention::Long
            } else {
                CacheRetention::Short
            }
        }
    }
}

// ============================================================================
// StreamAnthropic: main streaming function
// ============================================================================

/// Stream a completion from the Anthropic Messages API.
pub fn stream_anthropic(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> AssistantMessageEventStream {
    let model = model.clone();
    let context = context.clone();
    let owned_options = options.cloned();
    let api_key = owned_options
        .as_ref()
        .and_then(|o| o.api_key.clone())
        .or_else(|| crate::env_api_keys::get_env_api_key(&model.provider));

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        let result = stream_anthropic_inner(
            &model,
            &context,
            owned_options.as_ref(),
            api_key.as_deref(),
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
    tx: &tokio::sync::mpsc::UnboundedSender<AssistantMessageEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key = api_key.ok_or_else(|| format!("No API key for provider: {}", model.provider))?;

    let compat = get_anthropic_compat(model);
    let temperature = options.and_then(|o| o.temperature);
    let max_tokens = options
        .and_then(|o| o.max_tokens)
        .unwrap_or(model.max_tokens);
    let signal = options.and_then(|o| o.signal.clone());
    let _cache_retention = options.and_then(|o| o.cache_retention.as_ref());

    let http_client = HttpClient::builder()
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("x-api-key", api_key.parse().unwrap());
            headers.insert("anthropic-version", ANTHROPIC_VERSION.parse().unwrap());
            headers.insert("content-type", "application/json".parse().unwrap());
            if compat.supports_eager_tool_input_streaming.unwrap_or(true) {
                headers.insert(
                    "anthropic-beta",
                    FINE_GRAINED_TOOL_STREAMING_BETA.parse().unwrap(),
                );
            }
            if let Some(session_id) = options.and_then(|o| o.session_id.as_deref()) {
                if compat.send_session_affinity_headers.unwrap_or(false) {
                    headers.insert("x-session-affinity", session_id.parse().unwrap());
                }
            }
            headers
        })
        .build()?;

    // Build request body
    let messages = convert_messages(&context.messages, model);
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(model.id.clone()));
    body.insert("messages".to_string(), serde_json::to_value(&messages)?);
    body.insert("max_tokens".to_string(), Value::Number(max_tokens.into()));
    body.insert("stream".to_string(), Value::Bool(true));

    if let Some(ref sp) = context.system_prompt {
        body.insert("system".to_string(), Value::String(sp.clone()));
    }

    if let Some(t) = temperature {
        body.insert(
            "temperature".to_string(),
            Value::Number(serde_json::Number::from_f64(t).unwrap()),
        );
    }

    if let Some(ref tools) = context.tools {
        if !tools.is_empty() {
            body.insert(
                "tools".to_string(),
                serde_json::to_value(convert_tools(tools))?,
            );
        }
    }

    // Check for abort signal before making the request
    if let Some(ref rx) = signal {
        if *rx.borrow() {
            return Err("Request was aborted".into());
        }
    }

    let request_body = Value::Object(body);
    let response = http_client
        .post(&model.base_url)
        .json(&request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Anthropic API error {}: {}", status, text).into());
    }

    let response_bytes = response.bytes().await?;
    let sse_events = parse_sse_body(&response_bytes);

    // Initialize output
    let mut output = AssistantMessage {
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
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

    for sse in &sse_events {
        // Check for abort
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

        let data = &sse.data;
        if data.is_empty() {
            continue;
        }

        let event: AnthropicSseEvent = match serde_json::from_str(data) {
            Ok(e) => e,
            Err(_) => continue,
        };

        match event {
            AnthropicSseEvent::MessageStart { message } => {
                output.response_id = Some(message.id);
                output.response_model = Some(message.model);
                output.usage.input = message.usage.input_tokens.unwrap_or(0);
                output.usage.output = message.usage.output_tokens.unwrap_or(0);
                output.usage.cache_read = message.usage.cache_read_input_tokens.unwrap_or(0);
                output.usage.cache_write = message.usage.cache_creation_input_tokens.unwrap_or(0);
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
                        _ => {}
                    }
                }
            }
            AnthropicSseEvent::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.stop_reason {
                    output.stop_reason = map_stop_reason(&reason);
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

    // Calculate cost
    crate::models::calculate_cost(model, &mut output.usage);

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
pub fn stream_simple_anthropic(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let mut full_opts = StreamOptions::default();
    if let Some(opts) = options {
        full_opts.temperature = opts.base.temperature;
        full_opts.max_tokens = opts.base.max_tokens;
        full_opts.signal = opts.base.signal.clone();
        full_opts.api_key = opts.base.api_key.clone();
        full_opts.transport = opts.base.transport.clone();
        full_opts.cache_retention = opts.base.cache_retention.clone();
        full_opts.session_id = opts.base.session_id.clone();
        full_opts.headers = opts.base.headers.clone();
        full_opts.timeout_ms = opts.base.timeout_ms;
        full_opts.max_retries = opts.base.max_retries;
        full_opts.max_retry_delay_ms = opts.base.max_retry_delay_ms;
        full_opts.metadata = opts.base.metadata.clone();
    }
    stream_anthropic(model, context, Some(&full_opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ModelCost};

    fn make_test_model() -> Model {
        Model {
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
            },
            context_window: 200000,
            max_tokens: 8192,
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
        assert_eq!(map_stop_reason("end_turn"), StopReason::Stop);
    }

    #[test]
    fn test_map_stop_reason_max_tokens() {
        assert_eq!(map_stop_reason("max_tokens"), StopReason::Length);
    }

    #[test]
    fn test_map_stop_reason_tool_use() {
        assert_eq!(map_stop_reason("tool_use"), StopReason::ToolUse);
    }

    #[test]
    fn test_map_stop_reason_stop_sequence() {
        assert_eq!(map_stop_reason("stop_sequence"), StopReason::Stop);
    }

    #[test]
    fn test_map_stop_reason_unknown() {
        assert_eq!(map_stop_reason("unknown_reason"), StopReason::Stop);
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
        let converted = convert_messages(&messages, &model);
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
        let converted = convert_messages(&messages, &model);
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
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages, &model);
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
                timestamp: 1000,
            },
        ];
        let converted = convert_messages(&messages, &model);
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
        let converted = convert_messages(&messages, &model);
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
        let converted = convert_messages(&messages, &model);
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
        let converted = convert_messages(&messages, &model);
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
    fn test_convert_tools_empty() {
        let converted = convert_tools(&[]);
        assert!(converted.is_empty());
    }

    #[test]
    fn test_convert_tools_single() {
        let tools = vec![Tool {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        let converted = convert_tools(&tools);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].name, "read");
        assert_eq!(converted[0].description, "Read a file");
    }

    // ============================================================
    // resolve_cache_retention tests
    // ============================================================

    #[test]
    fn test_resolve_cache_retention_explicit() {
        let retention = resolve_cache_retention(Some(&CacheRetention::Long));
        assert_eq!(retention, CacheRetention::Long);
    }

    #[test]
    fn test_resolve_cache_retention_none() {
        let retention = resolve_cache_retention(Some(&CacheRetention::None));
        assert_eq!(retention, CacheRetention::None);
    }

    #[test]
    fn test_resolve_cache_retention_default() {
        // Default when not specified and no env var
        let retention = resolve_cache_retention(None);
        assert_eq!(retention, CacheRetention::Short);
    }
}
