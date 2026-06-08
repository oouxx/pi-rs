//! OpenAI Chat Completions API provider.
//!
//! Thin wrapper around the OpenAI Chat Completions API using reqwest for HTTP
//! and SSE streaming. Converts between pi-ai types and OpenAI API format.
//!
//! Ported from `packages/ai/src/providers/openai-completions.ts`.

use reqwest::Client as HttpClient;
use serde::Serialize;
use serde_json::Value;

use crate::types::{
    AssistantMessage, AssistantMessageEvent, ContentBlock, Context, Message, Model,
    SimpleStreamOptions, StopReason, StreamOptions, Tool, Usage,
};
use crate::utils::event_stream::AssistantMessageEventStream;

// ============================================================================
// OpenAI API types (request/response)
// ============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    tc_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionDef,
}

#[derive(Debug, Serialize)]
struct OpenAIFunctionDef {
    name: String,
    description: String,
    parameters: Value,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptionsFlag>,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct StreamOptionsFlag {
    include_usage: bool,
}

// ============================================================================
// Message conversion
// ============================================================================

/// Convert pi-ai messages to OpenAI API format.
fn convert_messages(messages: &[Message]) -> Vec<OpenAIMessage> {
    let mut result = Vec::new();

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
                    result.push(OpenAIMessage {
                        role: "user".to_string(),
                        content: Some(text),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }
            Message::Assistant { content, .. } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                let tool_calls: Vec<OpenAIToolCall> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => Some(OpenAIToolCall {
                            id: id.clone(),
                            tc_type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: name.clone(),
                                arguments: arguments.to_string(),
                            },
                        }),
                        _ => None,
                    })
                    .collect();

                if !text.is_empty() || !tool_calls.is_empty() {
                    result.push(OpenAIMessage {
                        role: "assistant".to_string(),
                        content: if text.is_empty() { None } else { Some(text) },
                        tool_calls: if tool_calls.is_empty() {
                            None
                        } else {
                            Some(tool_calls)
                        },
                        tool_call_id: None,
                    });
                }
            }
            Message::ToolResult {
                tool_call_id,
                content,
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

                result.push(OpenAIMessage {
                    role: "tool".to_string(),
                    content: Some(text),
                    tool_calls: None,
                    tool_call_id: Some(tool_call_id.clone()),
                });
            }
        }
    }

    result
}

/// Convert pi-ai tools to OpenAI tool definitions.
fn convert_tools(tools: &[Tool]) -> Vec<OpenAITool> {
    tools
        .iter()
        .map(|t| OpenAITool {
            tool_type: "function".to_string(),
            function: OpenAIFunctionDef {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
        })
        .collect()
}

/// Map OpenAI finish reason to pi-ai StopReason.
fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::Stop,
        "length" => StopReason::Length,
        "tool_calls" => StopReason::ToolUse,
        "content_filter" => StopReason::Error,
        _ => StopReason::Stop,
    }
}

// ============================================================================
// OpenAI SSE parsing (different format from Anthropic)
// ============================================================================

/// Parse a single line from the OpenAI SSE stream.
/// OpenAI SSE format is simpler: each line is `data: <json>`, ending with `data: [DONE]`.
fn parse_openai_sse_chunk(line: &str) -> Option<Value> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    // Extract data field
    let data = if let Some(rest) = line.strip_prefix("data: ") {
        rest
    } else if let Some(rest) = line.strip_prefix("data:") {
        rest
    } else {
        return None;
    };

    if data == "[DONE]" {
        return None; // end of stream marker
    }

    serde_json::from_str(data).ok()
}

/// Parse a complete OpenAI SSE response body into JSON values.
fn parse_openai_sse_body(body: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(body);
    let mut events = Vec::new();

    for line in text.lines() {
        if let Some(event) = parse_openai_sse_chunk(line) {
            events.push(event);
        }
    }

    events
}

/// Parse token usage from an OpenAI chunk.
fn parse_chunk_usage(usage: &Value) -> Usage {
    Usage {
        input: usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output: usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read: usage
            .get("prompt_tokens_details")
            .and_then(|v| v.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_write: 0,
        total_tokens: usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cost: Default::default(),
    }
}

// ============================================================================
// StreamOpenAI: main streaming function
// ============================================================================

/// Stream a completion from the OpenAI Chat Completions API.
pub fn stream_openai(
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
        let result = stream_openai_inner(
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

async fn stream_openai_inner(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
    api_key: Option<&str>,
    tx: &tokio::sync::mpsc::UnboundedSender<AssistantMessageEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key = api_key.ok_or_else(|| format!("No API key for provider: {}", model.provider))?;
    let max_tokens = options.and_then(|o| o.max_tokens);
    let temperature = options.and_then(|o| o.temperature);
    let signal = options.and_then(|o| o.signal.clone());

    let http_client = HttpClient::new();

    let messages = convert_messages(&context.messages);
    let tools = context
        .tools
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| convert_tools(t));

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(model.id.clone()));
    body.insert("messages".to_string(), serde_json::to_value(&messages)?);
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert(
        "stream_options".to_string(),
        serde_json::json!({"include_usage": true}),
    );

    if let Some(mt) = max_tokens {
        body.insert("max_tokens".to_string(), Value::Number(mt.into()));
    }
    if let Some(t) = temperature {
        body.insert(
            "temperature".to_string(),
            Value::Number(serde_json::Number::from_f64(t).unwrap_or(serde_json::Number::from(1))),
        );
    }
    if let Some(ref t) = tools {
        body.insert("tools".to_string(), serde_json::to_value(t)?);
    }
    if let Some(ref tc) = options.and_then(|o| o.tool_choice.as_ref()) {
        body.insert("tool_choice".to_string(), serde_json::to_value(tc)?);
    }

    // Check abort signal
    if let Some(ref rx) = signal {
        if *rx.borrow() {
            return Err("Request was aborted".into());
        }
    }

    let request_body = Value::Object(body);
    let response = http_client
        .post(format!(
            "{}/chat/completions",
            model.base_url.trim_end_matches('/')
        ))
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("OpenAI API error {}: {}", status, text).into());
    }

    let response_bytes = response.bytes().await?;
    let chunks = parse_openai_sse_body(&response_bytes);

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

    let mut has_finish_reason = false;

    // Track current streaming blocks
    struct ToolCallBlock {
        content_index: usize,
        id: String,
        name: String,
        partial_args: String,
    }

    let mut current_text: Option<(usize, String)> = None; // (content_index, text)
                                                          // Two lookup maps, both index into tool_call_blocks Vec
    let mut tool_call_blocks: Vec<ToolCallBlock> = Vec::new();
    let mut tool_call_blocks_by_index: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    let mut tool_call_blocks_by_id: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for chunk in &chunks {
        // Check abort
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

        // Capture response ID and model
        if output.response_id.is_none() {
            if let Some(id) = chunk.get("id").and_then(|v| v.as_str()) {
                output.response_id = Some(id.to_string());
            }
        }

        // Parse usage if present
        if let Some(usage) = chunk.get("usage") {
            output.usage = parse_chunk_usage(usage);
        }

        // Parse choices
        let choices = match chunk.get("choices").and_then(|v| v.as_array()) {
            Some(c) => c,
            None => continue,
        };

        if choices.is_empty() {
            continue;
        }
        let choice = &choices[0];

        // Check finish reason
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            if !reason.is_empty() {
                output.stop_reason = map_stop_reason(reason);
                if output.stop_reason == StopReason::Error {
                    output.error_message =
                        Some(format!("Provider returned finish_reason: {}", reason));
                }
                has_finish_reason = true;
            }
        }

        // Parse delta
        let delta = match choice.get("delta") {
            Some(d) => d,
            None => continue,
        };

        // Text content
        if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty() {
                if current_text.is_none() {
                    let ci = output.content.len();
                    output.content.push(ContentBlock::text(""));
                    let _ = tx.send(AssistantMessageEvent::TextStart {
                        content_index: ci,
                        partial: output.clone(),
                    });
                    current_text = Some((ci, String::new()));
                }
                if let Some((ci, ref mut text)) = current_text {
                    text.push_str(content);
                    if let Some(ContentBlock::Text {
                        text: ref mut t, ..
                    }) = output.content.get_mut(ci)
                    {
                        *t = text.clone();
                    }
                    let _ = tx.send(AssistantMessageEvent::TextDelta {
                        content_index: ci,
                        delta: content.to_string(),
                        partial: output.clone(),
                    });
                }
            }
        }

        // Reasoning/thinking content
        for field in &["reasoning_content", "reasoning", "reasoning_text"] {
            if let Some(reasoning) = delta.get(field).and_then(|v| v.as_str()) {
                if !reasoning.is_empty() {
                    // For simplicity, treat reasoning as thinking blocks
                    // Find or create a thinking block
                    let thinking_idx = output
                        .content
                        .iter()
                        .position(|b| matches!(b, ContentBlock::Thinking { .. }));
                    if let Some(ti) = thinking_idx {
                        if let Some(ContentBlock::Thinking {
                            thinking: ref mut t,
                            ..
                        }) = output.content.get_mut(ti)
                        {
                            t.push_str(reasoning);
                            let _ = tx.send(AssistantMessageEvent::ThinkingDelta {
                                content_index: ti,
                                delta: reasoning.to_string(),
                                partial: output.clone(),
                            });
                        }
                    } else {
                        let ci = output.content.len();
                        output.content.push(ContentBlock::Thinking {
                            thinking: reasoning.to_string(),
                            thinking_signature: Some(field.to_string()),
                            redacted: None,
                        });
                        let _ = tx.send(AssistantMessageEvent::ThinkingStart {
                            content_index: ci,
                            partial: output.clone(),
                        });
                    }
                }
            }
        }

        // Tool calls — using dual-map lookup (by index, by id) aligned with TS pi
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tc in tool_calls {
                let stream_index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let tc_id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let tc_function = tc.get("function");

                // Find or create tool call block (TS: ensureToolCallBlock)
                let block_idx = tool_call_blocks_by_index
                    .get(&stream_index)
                    .copied()
                    .or_else(|| {
                        if tc_id.is_empty() {
                            None
                        } else {
                            tool_call_blocks_by_id.get(tc_id).copied()
                        }
                    });

                if let Some(bi) = block_idx {
                    let block = &mut tool_call_blocks[bi];
                    if !tc_id.is_empty() && block.id.is_empty() {
                        block.id = tc_id.to_string();
                        tool_call_blocks_by_id.insert(block.id.clone(), bi);
                    }
                    if let Some(func) = tc_function {
                        if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                            if block.name.is_empty() {
                                block.name = name.to_string();
                            }
                        }
                        if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                            block.partial_args.push_str(args);
                            if let Ok(parsed) = serde_json::from_str::<Value>(&block.partial_args) {
                                if let Some(ContentBlock::ToolCall {
                                    arguments: ref mut a,
                                    ..
                                }) = output.content.get_mut(block.content_index)
                                {
                                    *a = parsed;
                                }
                            }
                            let _ = tx.send(AssistantMessageEvent::ToolCallDelta {
                                content_index: block.content_index,
                                delta: args.to_string(),
                                partial: output.clone(),
                            });
                        }
                    }
                } else {
                    let ci = output.content.len();
                    let mut name = String::new();
                    let mut first_args = String::new();
                    if let Some(func) = tc_function {
                        if let Some(n) = func.get("name").and_then(|v| v.as_str()) {
                            name = n.to_string();
                        }
                        if let Some(a) = func.get("arguments").and_then(|v| v.as_str()) {
                            first_args = a.to_string();
                        }
                    }
                    let args_val = serde_json::from_str::<Value>(&first_args)
                        .unwrap_or(Value::Object(Default::default()));

                    output.content.push(ContentBlock::ToolCall {
                        id: tc_id.to_string(),
                        name: name.clone(),
                        arguments: args_val,
                        thought_signature: None,
                    });
                    let _ = tx.send(AssistantMessageEvent::ToolCallStart {
                        content_index: ci,
                        partial: output.clone(),
                    });

                    let bi = tool_call_blocks.len();
                    tool_call_blocks.push(ToolCallBlock {
                        content_index: ci,
                        id: tc_id.to_string(),
                        name,
                        partial_args: first_args,
                    });
                    tool_call_blocks_by_index.insert(stream_index, bi);
                    if !tc_id.is_empty() {
                        tool_call_blocks_by_id.insert(tc_id.to_string(), bi);
                    }
                }
            }
        }
    }

    // Match TS: check that we received a finish_reason
    if !has_finish_reason {
        output.stop_reason = StopReason::Error;
        output.error_message = Some("Stream ended without finish_reason".to_string());
        let _ = tx.send(AssistantMessageEvent::Error {
            reason: StopReason::Error,
            error: output.clone(),
        });
        return Ok(());
    }

    // Finalize all blocks
    if let Some((ci, text)) = current_text {
        let _ = tx.send(AssistantMessageEvent::TextEnd {
            content_index: ci,
            content: text,
            partial: output.clone(),
        });
    }
    for block in &tool_call_blocks {
        if let Some(ContentBlock::ToolCall {
            id,
            name,
            arguments,
            ..
        }) = output.content.get(block.content_index)
        {
            let tool_call =
                crate::types::ToolCall::new(id.clone(), name.clone(), arguments.clone());
            let _ = tx.send(AssistantMessageEvent::ToolCallEnd {
                content_index: block.content_index,
                tool_call,
                partial: output.clone(),
            });
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
// streamSimpleOpenAI
// ============================================================================

/// Stream a completion from OpenAI with simplified options.
pub fn stream_simple_openai(
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
    stream_openai(model, context, Some(&full_opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelCost;

    fn make_test_model() -> Model {
        Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: "openai-completions".into(),
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into(), "image".into()],
            cost: ModelCost {
                input: 2.5,
                output: 10.0,
                cache_read: 1.25,
                cache_write: 0.0,
            },
            context_window: 128000,
            max_tokens: 16384,
            headers: None,
            compat: None,
        }
    }

    // ============================================================
    // map_stop_reason tests
    // ============================================================

    #[test]
    fn test_map_stop_reason_stop() {
        assert_eq!(map_stop_reason("stop"), StopReason::Stop);
    }

    #[test]
    fn test_map_stop_reason_length() {
        assert_eq!(map_stop_reason("length"), StopReason::Length);
    }

    #[test]
    fn test_map_stop_reason_tool_calls() {
        assert_eq!(map_stop_reason("tool_calls"), StopReason::ToolUse);
    }

    #[test]
    fn test_map_stop_reason_content_filter() {
        assert_eq!(map_stop_reason("content_filter"), StopReason::Error);
    }

    #[test]
    fn test_map_stop_reason_unknown() {
        assert_eq!(map_stop_reason("unknown"), StopReason::Stop);
    }

    // ============================================================
    // convert_messages tests
    // ============================================================

    #[test]
    fn test_convert_messages_user() {
        let messages = vec![Message::User {
            content: vec![ContentBlock::text("Hello")],
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "user");
        assert_eq!(converted[0].content, Some("Hello".to_string()));
    }

    #[test]
    fn test_convert_messages_assistant_with_tool_calls() {
        let messages = vec![Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "call_1".into(),
                name: "get_weather".into(),
                arguments: serde_json::json!({"city": "NYC"}),
                thought_signature: None,
            }],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-4o".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "assistant");
        assert!(converted[0].tool_calls.is_some());
        let tcs = converted[0].tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "get_weather");
    }

    #[test]
    fn test_convert_messages_tool_result() {
        let messages = vec![Message::ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "get_weather".into(),
            content: vec![ContentBlock::text("72F sunny")],
            details: None,
            is_error: false,
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "tool");
        assert_eq!(converted[0].tool_call_id, Some("call_1".to_string()));
    }

    #[test]
    fn test_convert_messages_empty_user() {
        let messages = vec![Message::User {
            content: vec![ContentBlock::text("")],
            timestamp: 1000,
        }];
        let converted = convert_messages(&messages);
        assert_eq!(converted.len(), 0);
    }

    // ============================================================
    // convert_tools tests
    // ============================================================

    #[test]
    fn test_convert_tools() {
        let tools = vec![Tool {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];
        let converted = convert_tools(&tools);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].tool_type, "function");
        assert_eq!(converted[0].function.name, "read");
    }

    // ============================================================
    // parse_openai_sse_chunk tests
    // ============================================================

    #[test]
    fn test_parse_sse_chunk_data_line() {
        let chunk = parse_openai_sse_chunk(
            r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"}}]}"#,
        );
        assert!(chunk.is_some());
        let val = chunk.unwrap();
        assert_eq!(val["id"], "chatcmpl-123");
    }

    #[test]
    fn test_parse_sse_chunk_done() {
        let chunk = parse_openai_sse_chunk("data: [DONE]");
        assert!(chunk.is_none());
    }

    #[test]
    fn test_parse_sse_chunk_empty() {
        assert!(parse_openai_sse_chunk("").is_none());
    }

    #[test]
    fn test_parse_sse_chunk_comment() {
        assert!(parse_openai_sse_chunk(": comment").is_none());
    }

    // ============================================================
    // parse_chunk_usage tests
    // ============================================================

    #[test]
    fn test_parse_chunk_usage() {
        let usage_json = serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_tokens": 150,
            "prompt_tokens_details": {"cached_tokens": 20}
        });
        let usage = parse_chunk_usage(&usage_json);
        assert_eq!(usage.input, 100);
        assert_eq!(usage.output, 50);
        assert_eq!(usage.total_tokens, 150);
        assert_eq!(usage.cache_read, 20);
    }
}
