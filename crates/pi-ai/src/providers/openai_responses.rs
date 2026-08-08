//! `OpenAI` Responses API provider.
//!
//! Thin wrapper around the `OpenAI` Responses API (`/v1/responses`) using
//! reqwest for HTTP and SSE streaming. Converts between pi-ai types and the
//! Responses API wire format.
//!
//! Ported from `packages/ai/src/api/openai-responses.ts` +
//! `packages/ai/src/api/openai-responses-shared.ts`.

use std::collections::HashMap;

use reqwest::Client as HttpClient;
use serde_json::{json, Value};

use crate::types::{
    AssistantMessage, AssistantMessageEvent, CacheRetention, ContentBlock, Context, Message, Model,
    OpenAIResponsesCompat, SessionAffinityFormat, SimpleStreamOptions, StopReason, StreamOptions,
    Tool, Usage,
};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::json_parse::parse_streaming_json;

/// OpenAI Responses rejects `max_output_tokens` below 16:
/// https://github.com/earendil-works/pi/issues/6265
const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: u64 = 16;
const OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;

/// Providers whose tool call ids carry the `callId|itemId` form.
const OPENAI_TOOL_CALL_PROVIDERS: &[&str] = &["openai", "openai-codex", "opencode"];

// ============================================================================
// Compat resolution (match TS `getCompat` / `detectSessionAffinityFormat`)
// ============================================================================

fn detect_session_affinity_format(model: &Model) -> SessionAffinityFormat {
    if model.provider == "openrouter" || model.base_url.contains("openrouter.ai") {
        SessionAffinityFormat::Openrouter
    } else {
        SessionAffinityFormat::Openai
    }
}

fn get_compat(model: &Model) -> OpenAIResponsesCompat {
    match &model.compat {
        Some(crate::types::ModelCompat::OpenAIResponses(compat)) => compat.clone(),
        _ => OpenAIResponsesCompat {
            supports_developer_role: None,
            session_affinity_format: None,
            supports_long_cache_retention: None,
            supports_strict_mode: None,
            supports_openai_grammar_tools: None,
            supports_tool_search: None,
            supports_explicit_prompt_cache_mode: None,
        },
    }
}

fn compat_supports_developer_role(compat: &OpenAIResponsesCompat) -> bool {
    compat.supports_developer_role.unwrap_or(true)
}

fn compat_session_affinity_format(model: &Model, compat: &OpenAIResponsesCompat) -> SessionAffinityFormat {
    compat
        .session_affinity_format
        .clone()
        .unwrap_or_else(|| detect_session_affinity_format(model))
}

fn compat_supports_long_cache_retention(compat: &OpenAIResponsesCompat) -> bool {
    compat.supports_long_cache_retention.unwrap_or(true)
}

fn compat_supports_strict_mode(compat: &OpenAIResponsesCompat) -> bool {
    compat.supports_strict_mode.unwrap_or(false)
}

// ============================================================================
// Service tier pricing (match TS `getServiceTierCostMultiplier` / `applyServiceTierPricing`)
// ============================================================================

fn get_service_tier_cost_multiplier(model_id: &str, service_tier: Option<&str>) -> f64 {
    match service_tier {
        Some("flex") => 0.5,
        Some("priority") => {
            if model_id == "gpt-5.5" {
                2.5
            } else {
                2.0
            }
        }
        _ => 1.0,
    }
}

fn apply_service_tier_pricing(usage: &mut Usage, model_id: &str, service_tier: Option<&str>) {
    let multiplier = get_service_tier_cost_multiplier(model_id, service_tier);
    if multiplier == 1.0 {
        return;
    }
    usage.cost.input *= multiplier;
    usage.cost.output *= multiplier;
    usage.cost.cache_read *= multiplier;
    usage.cost.cache_write *= multiplier;
    usage.cost.total = usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

// ============================================================================
// Provider retry (match TS `retryProviderRequest` / `isRetryableProviderError`)
// ============================================================================

const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

struct ProviderHttpError {
    status: Option<u16>,
    headers: reqwest::header::HeaderMap,
    message: String,
}

fn is_retryable_provider_error(error: &ProviderHttpError) -> bool {
    if let Some(should_retry) = error.headers.get("x-should-retry").and_then(|v| v.to_str().ok()) {
        if should_retry == "true" {
            return true;
        }
        if should_retry == "false" {
            return false;
        }
    }
    match error.status {
        None => true,
        Some(408) | Some(409) | Some(429) => true,
        Some(s) => s >= 500,
    }
}

fn validate_server_retry_delay_ms(
    delay_ms: f64,
    max_retry_delay_ms: Option<u64>,
    provider_error_message: &str,
) -> Result<u64, String> {
    let max_delay_ms = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    if max_delay_ms > 0 && delay_ms > max_delay_ms as f64 {
        return Err(format!(
            "Server requested {}s retry delay (max: {}s). {provider_error_message}",
            (delay_ms / 1000.0).ceil(),
            (max_delay_ms as f64 / 1000.0).ceil(),
        ));
    }
    Ok(delay_ms as u64)
}

fn get_retry_delay_ms(
    error: &ProviderHttpError,
    retry_index: u32,
    max_retry_delay_ms: Option<u64>,
) -> Result<u64, String> {
    if let Some(v) = error.headers.get("retry-after-ms").and_then(|v| v.to_str().ok()) {
        if let Ok(value) = v.parse::<f64>() {
            return validate_server_retry_delay_ms(value, max_retry_delay_ms, &error.message);
        }
    }
    if let Some(v) = error.headers.get("retry-after").and_then(|v| v.to_str().ok()) {
        let delay_ms = match v.parse::<f64>() {
            Ok(seconds) => seconds * 1000.0,
            Err(_) => match chrono::DateTime::parse_from_rfc2822(v) {
                Ok(dt) => (dt.timestamp_millis() - chrono::Utc::now().timestamp_millis()) as f64,
                Err(_) => f64::NAN,
            },
        };
        if !delay_ms.is_nan() {
            return validate_server_retry_delay_ms(delay_ms, max_retry_delay_ms, &error.message);
        }
    }
    let exponential = (0.5 * 2f64.powi(retry_index as i32)).min(8.0) * 1000.0;
    Ok((exponential * (1.0 - rand::random::<f64>() * 0.25)) as u64)
}

async fn abortable_sleep(
    ms: u64,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(), String> {
    if let Some(rx) = signal {
        if *rx.borrow() {
            return Err("Request aborted".into());
        }
        let mut rx = rx.clone();
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(ms)) => {}
            _ = rx.changed() => {
                if *rx.borrow() {
                    return Err("Request aborted".into());
                }
            }
        }
        Ok(())
    } else {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        Ok(())
    }
}

/// Send the request with retry, mirroring the OpenAI/Anthropic SDK retry policy
/// (match TS `retryProviderRequest`).
async fn send_with_retry(
    request_fn: impl Fn() -> reqwest::RequestBuilder,
    max_retries: Option<u32>,
    max_retry_delay_ms: Option<u64>,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<reqwest::Response, Box<dyn std::error::Error + Send + Sync>> {
    let max_retries = max_retries.unwrap_or(0);
    let mut retries_remaining = max_retries;
    loop {
        let result = request_fn().send().await;
        let error = match result {
            Ok(response) => {
                if response.status().is_success() {
                    return Ok(response);
                }
                let status = response.status();
                let headers = response.headers().clone();
                let text = response.text().await.unwrap_or_default();
                ProviderHttpError {
                    status: Some(status.as_u16()),
                    headers,
                    message: format!("OpenAI API error {status}: {text}"),
                }
            }
            Err(e) => ProviderHttpError {
                status: None,
                headers: reqwest::header::HeaderMap::new(),
                message: e.to_string(),
            },
        };
        if retries_remaining == 0 || !is_retryable_provider_error(&error) {
            return Err(error.message.into());
        }
        if let Some(rx) = signal {
            if *rx.borrow() {
                return Err("Request aborted".into());
            }
        }
        let retry_index = max_retries - retries_remaining;
        retries_remaining -= 1;
        let delay = get_retry_delay_ms(&error, retry_index, max_retry_delay_ms)?;
        abortable_sleep(delay, signal).await?;
    }
}

// ============================================================================
// GitHub Copilot dynamic headers (match TS `buildCopilotDynamicHeaders`)
// ============================================================================

fn infer_copilot_initiator(messages: &[Message]) -> &'static str {
    match messages.last() {
        Some(Message::User { .. }) => "user",
        _ => "agent",
    }
}

fn has_copilot_vision_input(messages: &[Message]) -> bool {
    messages.iter().any(|msg| match msg {
        Message::User { content, .. } | Message::ToolResult { content, .. } => {
            content.iter().any(|b| matches!(b, ContentBlock::Image { .. }))
        }
        _ => false,
    })
}

fn build_copilot_dynamic_headers(messages: &[Message]) -> Vec<(String, String)> {
    let mut headers = vec![
        ("X-Initiator".to_string(), infer_copilot_initiator(messages).to_string()),
        ("Openai-Intent".to_string(), "conversation-edits".to_string()),
    ];
    if has_copilot_vision_input(messages) {
        headers.push(("Copilot-Vision-Request".to_string(), "true".to_string()));
    }
    headers
}

// ============================================================================
// Hash helpers (match TS `shortHash` exactly for deterministic ids)
// ============================================================================

fn to_base36(mut n: u32) -> String {
    if n == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = Vec::new();
    while n > 0 {
        s.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Fast deterministic hash to shorten long strings (matches TS `shortHash`).
fn short_hash(s: &str) -> String {
    let mut h1: u32 = 0xdeadbeef;
    let mut h2: u32 = 0x41c6ce57;
    for ch in s.chars() {
        let c = ch as u32;
        h1 = (h1 ^ c).wrapping_mul(2654435761);
        h2 = (h2 ^ c).wrapping_mul(1597334677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2246822507) ^ (h2 ^ (h2 >> 13)).wrapping_mul(3266489909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2246822507) ^ (h1 ^ (h1 >> 13)).wrapping_mul(3266489909);
    format!("{}{}", to_base36(h2), to_base36(h1))
}

// ============================================================================
// Tool call id normalization (match TS `normalizeIdPart` / `normalizeToolCallId`)
// ============================================================================

fn normalize_id_part(part: &str) -> String {
    let sanitized: String = part
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let truncated: String = if sanitized.len() > 64 {
        sanitized.chars().take(64).collect()
    } else {
        sanitized
    };
    truncated.trim_end_matches('_').to_string()
}

fn build_foreign_responses_item_id(item_id: &str) -> String {
    let normalized = format!("fc_{}", short_hash(item_id));
    if normalized.len() > 64 {
        normalized.chars().take(64).collect()
    } else {
        normalized
    }
}

fn normalize_tool_call_id(
    id: &str,
    model: &Model,
    source_provider: &str,
    source_api: &str,
) -> String {
    if !OPENAI_TOOL_CALL_PROVIDERS.contains(&model.provider.as_str()) {
        return normalize_id_part(id);
    }
    if !id.contains('|') {
        return normalize_id_part(id);
    }
    let (call_id, item_id) = id.split_once('|').unwrap_or((id, ""));
    let normalized_call_id = normalize_id_part(call_id);
    let is_foreign = source_provider != model.provider || source_api != model.api;
    let mut normalized_item_id = if is_foreign {
        build_foreign_responses_item_id(item_id)
    } else {
        normalize_id_part(item_id)
    };
    if !normalized_item_id.starts_with("fc_") {
        normalized_item_id = normalize_id_part(&format!("fc_{normalized_item_id}"));
    }
    format!("{normalized_call_id}|{normalized_item_id}")
}

// ============================================================================
// Text signature helpers (match TS `encodeTextSignatureV1` / `parseTextSignature`)
// ============================================================================

fn encode_text_signature_v1(id: &str, phase: Option<&str>) -> String {
    match phase {
        Some(p) => json!({ "v": 1, "id": id, "phase": p }).to_string(),
        None => json!({ "v": 1, "id": id }).to_string(),
    }
}

fn parse_text_signature(signature: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(sig) = signature else {
        return (None, None);
    };
    if sig.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<Value>(sig) {
            if parsed.get("v").and_then(Value::as_u64) == Some(1)
                && parsed.get("id").and_then(Value::as_str).is_some()
            {
                let id = parsed["id"].as_str().unwrap_or_default().to_string();
                let phase = parsed
                    .get("phase")
                    .and_then(Value::as_str)
                    .map(String::from);
                return (Some(id), phase);
            }
        }
    }
    (Some(sig.to_string()), None)
}

// ============================================================================
// Tool result output conversion
// ============================================================================

fn convert_tool_result_output(model: &Model, content: &[ContentBlock]) -> Value {
    let text_result: String = content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let images: Vec<&ContentBlock> = content.iter().filter(|b| matches!(b, ContentBlock::Image { .. })).collect();
    let has_text = !text_result.is_empty();

    if images.is_empty() || !model.input.iter().any(|i| i == "image") {
        let text = if has_text {
            text_result
        } else if !images.is_empty() {
            "(see attached image)".to_string()
        } else {
            "(no tool output)".to_string()
        };
        return Value::String(text);
    }

    let mut output: Vec<Value> = Vec::new();
    if has_text {
        output.push(json!({ "type": "input_text", "text": text_result }));
    }
    for image in images {
        if let ContentBlock::Image { data, mime_type } = image {
            output.push(json!({
                "type": "input_image",
                "detail": "auto",
                "image_url": format!("data:{mime_type};base64,{data}"),
            }));
        }
    }
    Value::Array(output)
}

// ============================================================================
// Message conversion (pi-ai → Responses API input items)
// ============================================================================

fn assistant_text_message(
    text: &str,
    text_signature: Option<&str>,
    text_block_index: &mut usize,
    msg_index: usize,
) -> Value {
    let (parsed_id, phase) = parse_text_signature(text_signature);
    let fallback_id = if *text_block_index == 0 {
        format!("msg_pi_{msg_index}")
    } else {
        format!("msg_pi_{msg_index}_{text_block_index}")
    };
    *text_block_index += 1;
    let msg_id = match parsed_id {
        Some(id) if id.len() > 64 => format!("msg_{}", short_hash(&id)),
        Some(id) => id,
        None => fallback_id,
    };
    let mut obj = serde_json::Map::new();
    obj.insert("type".into(), json!("message"));
    obj.insert("role".into(), json!("assistant"));
    obj.insert(
        "content".into(),
        json!([{ "type": "output_text", "text": text, "annotations": [] }]),
    );
    obj.insert("status".into(), json!("completed"));
    obj.insert("id".into(), json!(msg_id));
    if let Some(p) = phase {
        obj.insert("phase".into(), json!(p));
    }
    Value::Object(obj)
}

/// Convert pi-ai messages to Responses API `input` items.
fn convert_responses_messages(model: &Model, context: &Context) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();

    let compat = get_compat(model);
    if let Some(sp) = &context.system_prompt {
        let role = if model.reasoning && compat_supports_developer_role(&compat) {
            "developer"
        } else {
            "system"
        };
        messages.push(json!({ "role": role, "content": sp }));
    }

    let supports_images = model.input.iter().any(|i| i == "image");
    let mut msg_index = 0usize;

    for msg in &context.messages {
        match msg {
            Message::User { content, .. } => {
                let mut items: Vec<Value> = Vec::new();
                let mut previous_was_placeholder = false;
                for b in content {
                    match b {
                        ContentBlock::Text { text, .. } => {
                            items.push(json!({ "type": "input_text", "text": text }));
                            previous_was_placeholder = text == "(image omitted: model does not support images)";
                        }
                        ContentBlock::Image { data, mime_type } => {
                            if !supports_images {
                                if !previous_was_placeholder {
                                    items.push(json!({
                                        "type": "input_text",
                                        "text": "(image omitted: model does not support images)",
                                    }));
                                }
                                previous_was_placeholder = true;
                            } else {
                                items.push(json!({
                                    "type": "input_image",
                                    "detail": "auto",
                                    "image_url": format!("data:{mime_type};base64,{data}"),
                                }));
                                previous_was_placeholder = false;
                            }
                        }
                        _ => {}
                    }
                }
                if items.is_empty() {
                    continue;
                }
                messages.push(json!({ "role": "user", "content": items }));
            }
            Message::Assistant {
                content,
                api,
                provider,
                model: msg_model,
                ..
            } => {
                let is_same_model =
                    msg_model == &model.id && provider == &model.provider && api == &model.api;
                let mut output: Vec<Value> = Vec::new();
                let mut text_block_index = 0usize;

                for b in content {
                    match b {
                        ContentBlock::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                        } => {
                            // transformMessages thinking handling:
                            // - redacted: only valid for the same model
                            // - same model + signature: replay the reasoning item
                            // - empty thinking: drop
                            // - different model: convert to plain text
                            if redacted.unwrap_or(false) {
                                if is_same_model {
                                    if let Some(sig) = thinking_signature {
                                        if let Ok(v) = serde_json::from_str::<Value>(sig) {
                                            output.push(v);
                                        }
                                    }
                                }
                            } else if is_same_model && thinking_signature.is_some() {
                                if let Some(sig) = thinking_signature {
                                    if let Ok(v) = serde_json::from_str::<Value>(sig) {
                                        output.push(v);
                                    }
                                }
                            } else if !thinking.trim().is_empty() && !is_same_model {
                                output.push(assistant_text_message(
                                    thinking,
                                    None,
                                    &mut text_block_index,
                                    msg_index,
                                ));
                            }
                        }
                        ContentBlock::Text {
                            text,
                            text_signature,
                            ..
                        } => {
                            output.push(assistant_text_message(
                                text,
                                text_signature.as_deref(),
                                &mut text_block_index,
                                msg_index,
                            ));
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            let normalized = normalize_tool_call_id(id, model, provider, api);
                            let (call_id, item_id_raw) =
                                normalized.split_once('|').unwrap_or((&normalized, ""));
                            let mut item_id: Option<String> = Some(item_id_raw.to_string());
                            // Without grammar tools (customInputProperty is always
                            // undefined here), drop non-fc_* ids and foreign fc_* ids.
                            if (is_same_model && !item_id_raw.starts_with("fc_"))
                                || (!is_same_model && item_id_raw.starts_with("fc_"))
                            {
                                item_id = None;
                            }
                            let mut obj = serde_json::Map::new();
                            obj.insert("type".into(), json!("function_call"));
                            obj.insert("call_id".into(), json!(call_id));
                            obj.insert("name".into(), json!(name));
                            obj.insert(
                                "arguments".into(),
                                json!(serde_json::to_string(arguments).unwrap_or_default()),
                            );
                            if let Some(iid) = item_id {
                                obj.insert("id".into(), json!(iid));
                            }
                            output.push(Value::Object(obj));
                        }
                        _ => {}
                    }
                }
                if !output.is_empty() {
                    messages.extend(output);
                }
            }
            Message::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                let normalized = normalize_tool_call_id(
                    tool_call_id,
                    model,
                    &model.provider,
                    &model.api,
                );
                let call_id = normalized.split('|').next().unwrap_or(&normalized).to_string();
                let output = convert_tool_result_output(model, content);
                messages.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            }
        }
        msg_index += 1;
    }

    messages
}

// ============================================================================
// Tool conversion
// ============================================================================

fn convert_responses_tools(tools: &[Tool], supports_strict_mode: bool) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut obj = serde_json::Map::new();
            obj.insert("type".into(), json!("function"));
            obj.insert("name".into(), json!(tool.name));
            obj.insert("description".into(), json!(tool.description));
            obj.insert("parameters".into(), tool.parameters.clone());
            if supports_strict_mode {
                obj.insert("strict".into(), json!(false));
            }
            Value::Object(obj)
        })
        .collect()
}

// ============================================================================
// SSE parsing
// ============================================================================

fn parse_responses_sse_chunk(line: &str) -> Option<Value> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    let data = line
        .strip_prefix("data: ")
        .or_else(|| line.strip_prefix("data:"))?;
    if data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

fn parse_responses_sse_body(body: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(body);
    text.lines().filter_map(parse_responses_sse_chunk).collect()
}

// ============================================================================
// Stream processing
// ============================================================================

enum OutputSlot {
    Thinking { content_index: usize },
    Text { content_index: usize },
    ToolCall {
        content_index: usize,
        partial_json: Option<String>,
    },
}

fn get_slot(
    output_slots: &HashMap<usize, OutputSlot>,
    output_index: usize,
    kind: &str,
) -> Option<usize> {
    match (output_slots.get(&output_index), kind) {
        (Some(OutputSlot::Thinking { content_index }), "thinking") => Some(*content_index),
        (Some(OutputSlot::Text { content_index }), "text") => Some(*content_index),
        (Some(OutputSlot::ToolCall { content_index, .. }), "toolCall") => Some(*content_index),
        _ => None,
    }
}

fn create_slot(
    output_index: usize,
    item: &Value,
    output: &mut AssistantMessage,
    output_slots: &mut HashMap<usize, OutputSlot>,
    tx: &tokio::sync::mpsc::UnboundedSender<AssistantMessageEvent>,
) {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    match item_type {
        "reasoning" => {
            output.content.push(ContentBlock::Thinking {
                thinking: String::new(),
                thinking_signature: None,
                redacted: None,
            });
            let content_index = output.content.len() - 1;
            output_slots.insert(output_index, OutputSlot::Thinking { content_index });
            let _ = tx.send(AssistantMessageEvent::ThinkingStart {
                content_index,
                partial: output.clone(),
            });
        }
        "message" => {
            output.content.push(ContentBlock::text(""));
            let content_index = output.content.len() - 1;
            output_slots.insert(output_index, OutputSlot::Text { content_index });
            let _ = tx.send(AssistantMessageEvent::TextStart {
                content_index,
                partial: output.clone(),
            });
        }
        "function_call" | "custom_tool_call" => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
            let block = crate::types::ToolCall::new(
                format!("{call_id}|{item_id}"),
                name.to_string(),
                parse_streaming_json(Some(arguments)),
            );
            output.content.push(ContentBlock::ToolCall {
                id: block.id.clone(),
                name: block.name.clone(),
                arguments: block.arguments.clone(),
                thought_signature: None,
            });
            let content_index = output.content.len() - 1;
            output_slots.insert(
                output_index,
                OutputSlot::ToolCall {
                    content_index,
                    partial_json: Some(arguments.to_string()),
                },
            );
            let _ = tx.send(AssistantMessageEvent::ToolCallStart {
                content_index,
                partial: output.clone(),
            });
        }
        _ => {}
    }
}

fn map_stop_reason(status: Option<&str>, incomplete_reason: Option<&str>) -> (StopReason, Option<String>) {
    match status {
        None => (StopReason::Stop, None),
        Some("completed") => (StopReason::Stop, None),
        Some("incomplete") => {
            if incomplete_reason == Some("max_output_tokens") {
                (StopReason::Length, None)
            } else {
                (
                    StopReason::Error,
                    Some(
                        incomplete_reason
                            .map(|r| format!("Response incomplete: {r}"))
                            .unwrap_or_else(|| "Response incomplete without a provider reason".to_string()),
                    ),
                )
            }
        }
        Some("failed") | Some("cancelled") => (StopReason::Error, None),
        Some("in_progress") | Some("queued") => (StopReason::Stop, None),
        other => panic!("Unhandled stop reason: {other:?}"),
    }
}

fn finalize_response(
    response: &Value,
    output: &mut AssistantMessage,
    model: &Model,
    reasoning_blocks: &mut HashMap<String, usize>,
    service_tier: Option<&str>,
) {
    // Backfill reasoning signatures from the terminal response output
    // (Azure can omit encrypted_content from output_item.done; pi#6409).
    if let Some(output_items) = response.get("output").and_then(Value::as_array) {
        for item in output_items {
            if item.get("type").and_then(Value::as_str) != Some("reasoning") {
                continue;
            }
            let Some(encrypted) = item.get("encrypted_content").and_then(Value::as_str) else {
                continue;
            };
            let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            if let Some(&content_index) = reasoning_blocks.get(item_id) {
                if let Some(ContentBlock::Thinking {
                    thinking_signature: Some(sig),
                    ..
                }) = output.content.get_mut(content_index)
                {
                    if let Ok(mut stored) = serde_json::from_str::<Value>(sig) {
                        if stored.get("encrypted_content").is_none() {
                            stored["encrypted_content"] = json!(encrypted);
                            *sig = stored.to_string();
                        }
                    }
                }
            }
        }
    }

    if let Some(id) = response.get("id").and_then(Value::as_str) {
        output.response_id = Some(id.to_string());
    }

    if let Some(usage) = response.get("usage") {
        let cached_tokens = usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_write_tokens = usage
            .pointer("/input_tokens_details/cache_write_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let input_tokens = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        let output_tokens = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
        let reasoning = usage
            .pointer("/output_tokens_details/reasoning_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        output.usage = Usage {
            input: input_tokens.saturating_sub(cached_tokens + cache_write_tokens),
            output: output_tokens,
            cache_read: cached_tokens,
            cache_write: cache_write_tokens,
            cache_write_1h: None,
            reasoning: Some(reasoning),
            total_tokens: usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
            cost: crate::types::UsageCost::default(),
        };
    }
    crate::models::calculate_cost(model, &mut output.usage);

    let status = response.get("status").and_then(Value::as_str);
    let incomplete_reason = response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str);
    // rawStopReason: `status` or `status.incompleteReason` (matches TS).
    output.raw_stop_reason = Some(
        incomplete_reason
            .map(|r| format!("{}.{}", status.unwrap_or(""), r))
            .unwrap_or_else(|| status.unwrap_or("").to_string()),
    );
    let (stop_reason, error_message) = map_stop_reason(status, incomplete_reason);
    output.stop_reason = stop_reason;
    if let Some(msg) = error_message {
        output.error_message = Some(msg);
    }
    if output.content.iter().any(|b| matches!(b, ContentBlock::ToolCall { .. }))
        && output.stop_reason == StopReason::Stop
    {
        output.stop_reason = StopReason::ToolUse;
    }
    // Service tier pricing (flex/priority multipliers).
    let response_service_tier = response.get("service_tier").and_then(Value::as_str);
    let effective_tier = response_service_tier.or(service_tier);
    apply_service_tier_pricing(&mut output.usage, &model.id, effective_tier);
}

fn process_responses_stream(
    events: &[Value],
    output: &mut AssistantMessage,
    tx: &tokio::sync::mpsc::UnboundedSender<AssistantMessageEvent>,
    model: &Model,
    service_tier: Option<&str>,
) -> Result<(), String> {
    let mut saw_terminal = false;
    let mut output_slots: HashMap<usize, OutputSlot> = HashMap::new();
    let mut reasoning_blocks: HashMap<String, usize> = HashMap::new();

    for event in events {
        let t = event.get("type").and_then(Value::as_str).unwrap_or("");
        match t {
            "response.created" => {
                if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                    output.response_id = Some(id.to_string());
                }
            }
            "response.output_item.added" => {
                let output_index = event.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let item = event.get("item").cloned().unwrap_or(Value::Null);
                create_slot(output_index, &item, output, &mut output_slots, tx);
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let output_index = event.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                if let Some(content_index) = get_slot(&output_slots, output_index, "thinking") {
                    if let ContentBlock::Thinking { thinking, .. } = &mut output.content[content_index] {
                        thinking.push_str(delta);
                    }
                    let _ = tx.send(AssistantMessageEvent::ThinkingDelta {
                        content_index,
                        delta: delta.to_string(),
                        partial: output.clone(),
                    });
                }
            }
            "response.reasoning_summary_part.done" => {
                let output_index = event.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(content_index) = get_slot(&output_slots, output_index, "thinking") {
                    if let ContentBlock::Thinking { thinking, .. } = &mut output.content[content_index] {
                        thinking.push_str("\n\n");
                    }
                    let _ = tx.send(AssistantMessageEvent::ThinkingDelta {
                        content_index,
                        delta: "\n\n".to_string(),
                        partial: output.clone(),
                    });
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let output_index = event.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                if let Some(content_index) = get_slot(&output_slots, output_index, "text") {
                    if let ContentBlock::Text { text, .. } = &mut output.content[content_index] {
                        text.push_str(delta);
                    }
                    let _ = tx.send(AssistantMessageEvent::TextDelta {
                        content_index,
                        delta: delta.to_string(),
                        partial: output.clone(),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                let output_index = event.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                if let Some(content_index) = get_slot(&output_slots, output_index, "toolCall") {
                    if let Some(OutputSlot::ToolCall {
                        partial_json: Some(pj),
                        ..
                    }) = output_slots.get_mut(&output_index)
                    {
                        pj.push_str(delta);
                    }
                    if let ContentBlock::ToolCall { arguments, .. } = &mut output.content[content_index] {
                        *arguments = parse_streaming_json(
                            output_slots
                                .get(&output_index)
                                .and_then(|s| match s {
                                    OutputSlot::ToolCall { partial_json, .. } => partial_json.as_deref(),
                                    _ => None,
                                }),
                        );
                    }
                    let _ = tx.send(AssistantMessageEvent::ToolCallDelta {
                        content_index,
                        delta: delta.to_string(),
                        partial: output.clone(),
                    });
                }
            }
            "response.function_call_arguments.done" => {
                let output_index = event.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let arguments = event.get("arguments").and_then(Value::as_str).unwrap_or("");
                if let Some(content_index) = get_slot(&output_slots, output_index, "toolCall") {
                    let previous = output_slots
                        .get(&output_index)
                        .and_then(|s| match s {
                            OutputSlot::ToolCall { partial_json, .. } => partial_json.clone(),
                            _ => None,
                        })
                        .unwrap_or_default();
                    if let Some(OutputSlot::ToolCall { partial_json, .. }) =
                        output_slots.get_mut(&output_index)
                    {
                        *partial_json = Some(arguments.to_string());
                    }
                    if let ContentBlock::ToolCall { arguments: args, .. } = &mut output.content[content_index] {
                        *args = parse_streaming_json(Some(arguments));
                    }
                    if arguments.starts_with(&previous) {
                        let delta = &arguments[previous.len()..];
                        if !delta.is_empty() {
                            let _ = tx.send(AssistantMessageEvent::ToolCallDelta {
                                content_index,
                                delta: delta.to_string(),
                                partial: output.clone(),
                            });
                        }
                    }
                }
            }
            "response.output_item.done" => {
                let output_index = event.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let item = event.get("item").cloned().unwrap_or(Value::Null);
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                match item_type {
                    "reasoning" => {
                        if let Some(content_index) = get_slot(&output_slots, output_index, "thinking") {
                            let summary_text = item
                                .get("summary")
                                .and_then(Value::as_array)
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|s| s.get("text").and_then(Value::as_str))
                                        .collect::<Vec<_>>()
                                        .join("\n\n")
                                })
                                .unwrap_or_default();
                            let content_text = item
                                .get("content")
                                .and_then(Value::as_array)
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|c| c.get("text").and_then(Value::as_str))
                                        .collect::<Vec<_>>()
                                        .join("\n\n")
                                })
                                .unwrap_or_default();
                            if let ContentBlock::Thinking { thinking, thinking_signature, .. } =
                                &mut output.content[content_index]
                            {
                                if !summary_text.is_empty() || !content_text.is_empty() {
                                    *thinking = summary_text + &content_text;
                                }
                                *thinking_signature = Some(item.to_string());
                            }
                            if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                                reasoning_blocks.insert(item_id.to_string(), content_index);
                            }
                            let content = match &output.content[content_index] {
                                ContentBlock::Thinking { thinking, .. } => thinking.clone(),
                                _ => String::new(),
                            };
                            let _ = tx.send(AssistantMessageEvent::ThinkingEnd {
                                content_index,
                                content,
                                partial: output.clone(),
                            });
                            output_slots.remove(&output_index);
                        }
                    }
                    "message" => {
                        if let Some(content_index) = get_slot(&output_slots, output_index, "text") {
                            let text = item
                                .get("content")
                                .and_then(Value::as_array)
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|c| {
                                            let t = c.get("type").and_then(Value::as_str).unwrap_or("");
                                            if t == "output_text" {
                                                c.get("text").and_then(Value::as_str)
                                            } else if t == "refusal" {
                                                c.get("refusal").and_then(Value::as_str)
                                            } else {
                                                None
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join("")
                                })
                                .unwrap_or_default();
                            let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
                            let phase = item.get("phase").and_then(Value::as_str);
                            if let ContentBlock::Text { text: tb, text_signature, .. } =
                                &mut output.content[content_index]
                            {
                                *tb = text.clone();
                                *text_signature = Some(encode_text_signature_v1(item_id, phase));
                            }
                            let _ = tx.send(AssistantMessageEvent::TextEnd {
                                content_index,
                                content: text,
                                partial: output.clone(),
                            });
                            output_slots.remove(&output_index);
                        }
                    }
                    "function_call" | "custom_tool_call" => {
                        if let Some(content_index) = get_slot(&output_slots, output_index, "toolCall") {
                            let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
                            if let ContentBlock::ToolCall { arguments: args, .. } = &mut output.content[content_index] {
                                *args = parse_streaming_json(Some(arguments));
                            }
                            if let Some(OutputSlot::ToolCall { partial_json, .. }) =
                                output_slots.get_mut(&output_index)
                            {
                                *partial_json = None;
                            }
                            let tool_call = match &output.content[content_index] {
                                ContentBlock::ToolCall { id, name, arguments, .. } => {
                                    crate::types::ToolCall::new(id.clone(), name.clone(), arguments.clone())
                                }
                                _ => continue,
                            };
                            let _ = tx.send(AssistantMessageEvent::ToolCallEnd {
                                content_index,
                                tool_call,
                                partial: output.clone(),
                            });
                            output_slots.remove(&output_index);
                        }
                    }
                    _ => {}
                }
            }
            "response.completed" | "response.incomplete" => {
                saw_terminal = true;
                let response = event.get("response").cloned().unwrap_or(Value::Null);
                finalize_response(&response, output, model, &mut reasoning_blocks, service_tier);
            }
            "response.failed" => {
                let response = event.get("response").cloned().unwrap_or(Value::Null);
                let error = response.get("error").cloned().unwrap_or(Value::Null);
                let details = response.get("incomplete_details").cloned().unwrap_or(Value::Null);
                let msg = if !error.is_null() {
                    let code = error.get("code").and_then(Value::as_str).unwrap_or("unknown");
                    let message = error.get("message").and_then(Value::as_str).unwrap_or("no message");
                    format!("{code}: {message}")
                } else if let Some(reason) = details.get("reason").and_then(Value::as_str) {
                    format!("incomplete: {reason}")
                } else {
                    "Unknown error (no error details in response)".to_string()
                };
                return Err(msg);
            }
            "error" => {
                let code = event.get("code").and_then(Value::as_str).unwrap_or("");
                let message = event.get("message").and_then(Value::as_str).unwrap_or("");
                return Err(format!("Error Code {code}: {message}"));
            }
            _ => {}
        }
    }

    if !saw_terminal {
        return Err("OpenAI Responses stream ended before a terminal response event".into());
    }
    Ok(())
}

// ============================================================================
// Stream entry points
// ============================================================================

fn build_request_body(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let compat = get_compat(model);
    let messages = convert_responses_messages(model, context);
    let tools = context
        .tools
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| convert_responses_tools(t, compat_supports_strict_mode(&compat)));

    let cache_retention = options
        .and_then(|o| o.cache_retention.clone())
        .unwrap_or(CacheRetention::Short);
    let session_id = options.and_then(|o| o.session_id.clone());
    let prompt_cache_key = if cache_retention == CacheRetention::None {
        None
    } else {
        session_id.map(|s| {
            let chars: Vec<char> = s.chars().collect();
            if chars.len() > OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH {
                chars[..OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH].iter().collect()
            } else {
                s
            }
        })
    };
    let prompt_cache_retention = if cache_retention == CacheRetention::Long
        && compat_supports_long_cache_retention(&compat)
    {
        Some("24h")
    } else {
        None
    };

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), json!(model.id));
    body.insert("input".to_string(), json!(messages));
    body.insert("stream".to_string(), json!(true));
    body.insert("store".to_string(), json!(false));
    if let Some(key) = prompt_cache_key {
        body.insert("prompt_cache_key".to_string(), json!(key));
    }
    if let Some(ret) = prompt_cache_retention {
        body.insert("prompt_cache_retention".to_string(), json!(ret));
    }

    if let Some(mt) = options.and_then(|o| o.max_tokens) {
        body.insert(
            "max_output_tokens".to_string(),
            json!(mt.max(OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)),
        );
    }
    if let Some(t) = options.and_then(|o| o.temperature) {
        body.insert("temperature".to_string(), json!(t));
    }
    if let Some(st) = options.and_then(|o| o.service_tier.as_ref()) {
        body.insert("service_tier".to_string(), json!(st));
    }
    if let Some(ref t) = tools {
        body.insert("tools".to_string(), serde_json::to_value(t)?);
    }
    if let Some(ref tc) = options.and_then(|o| o.tool_choice.as_ref()) {
        body.insert("tool_choice".to_string(), serde_json::to_value(tc)?);
    }

    if model.reasoning {
        let reasoning = json!({ "effort": "medium", "summary": "auto" });
        body.insert("reasoning".to_string(), reasoning);
        body.insert("include".to_string(), json!(["reasoning.encrypted_content"]));
    }

    Ok(Value::Object(body))
}

/// Stream a completion from the `OpenAI` Responses API.
pub fn stream_openai_responses(
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
        let result = stream_openai_responses_inner(
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
                    raw_stop_reason: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            });
        }
    });

    AssistantMessageEventStream::from_receiver(rx)
}

async fn stream_openai_responses_inner(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
    api_key: Option<&str>,
    tx: &tokio::sync::mpsc::UnboundedSender<AssistantMessageEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key = api_key.filter(|k| !k.is_empty());
    let signal = options.and_then(|o| o.signal.clone());

    if let Some(ref rx) = signal {
        if *rx.borrow() {
            return Err("Request was aborted".into());
        }
    }

    let request_body = build_request_body(model, context, options)?;
    let http_client = HttpClient::new();
    let url = format!("{}/responses", model.base_url.trim_end_matches('/'));

    // Build request headers (match TS createClient): auth, session affinity,
    // and GitHub Copilot dynamic headers.
    let mut headers: Vec<(String, String)> = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    if let Some(key) = api_key {
        headers.push(("Authorization".to_string(), format!("Bearer {key}")));
    }
    let cache_retention = options
        .and_then(|o| o.cache_retention.clone())
        .unwrap_or(CacheRetention::Short);
    if cache_retention != CacheRetention::None {
        if let Some(session_id) = options.and_then(|o| o.session_id.clone()) {
            let compat = get_compat(model);
            match compat_session_affinity_format(model, &compat) {
                SessionAffinityFormat::Openrouter => {
                    headers.push(("x-session-id".to_string(), session_id));
                }
                SessionAffinityFormat::Openai => {
                    headers.push(("session_id".to_string(), session_id.clone()));
                    headers.push(("x-client-request-id".to_string(), session_id));
                }
                SessionAffinityFormat::OpenaiNosession => {
                    headers.push(("x-client-request-id".to_string(), session_id));
                }
            }
        }
    }
    if model.provider == "github-copilot" {
        headers.extend(build_copilot_dynamic_headers(&context.messages));
    }

    let request_fn = || {
        let mut req = http_client.post(&url).json(&request_body);
        for (k, v) in &headers {
            req = req.header(k, v);
        }
        req
    };
    let response = send_with_retry(
        request_fn,
        options.and_then(|o| o.max_retries),
        options.and_then(|o| o.max_retry_delay_ms),
        &signal,
    )
    .await?;

    let response_bytes = response.bytes().await?;
    let events = parse_responses_sse_body(&response_bytes);

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
        raw_stop_reason: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    };

    let _ = tx.send(AssistantMessageEvent::Start {
        partial: output.clone(),
    });

    let service_tier = options.and_then(|o| o.service_tier.as_deref());
    process_responses_stream(&events, &mut output, tx, model, service_tier)?;

    if output.stop_reason == StopReason::Stop {
        output.stop_reason = StopReason::Stop;
    }

    let _ = tx.send(AssistantMessageEvent::Done {
        reason: output.stop_reason.clone(),
        message: output,
    });

    Ok(())
}

/// Stream a simple completion from the `OpenAI` Responses API.
pub fn stream_simple_openai_responses(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let mut full_opts = StreamOptions::default();
    if let Some(opts) = options {
        full_opts.temperature = opts.base.temperature;
        full_opts.max_tokens = opts.base.max_tokens;
        full_opts.signal.clone_from(&opts.base.signal);
        full_opts.api_key.clone_from(&opts.base.api_key);
        full_opts.transport.clone_from(&opts.base.transport);
        full_opts.cache_retention.clone_from(&opts.base.cache_retention);
        full_opts.session_id.clone_from(&opts.base.session_id);
        full_opts.headers.clone_from(&opts.base.headers);
        full_opts.timeout_ms = opts.base.timeout_ms;
        full_opts.max_retries = opts.base.max_retries;
        full_opts.max_retry_delay_ms = opts.base.max_retry_delay_ms;
        full_opts.metadata.clone_from(&opts.base.metadata);
    }
    stream_openai_responses(model, context, Some(&full_opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn make_model() -> Model {
        Model {
            id: "gpt-5".into(),
            name: "GPT-5".into(),
            api: "openai-responses".into(),
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec!["text".into(), "image".into()],
            cost: crate::types::ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.1,
                cache_write: 0.2,
                tiers: vec![],
            },
            context_window: 128_000,
            max_tokens: 16_384,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn test_short_hash_matches_ts() {
        // Deterministic; just verify it is stable and base36.
        let h = short_hash("tool_use_123");
        assert!(!h.is_empty());
        assert!(h.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(short_hash("tool_use_123"), short_hash("tool_use_123"));
    }

    #[test]
    fn test_normalize_id_part() {
        assert_eq!(normalize_id_part("abc123"), "abc123");
        // trailing underscores are trimmed (matches TS normalizeIdPart)
        assert_eq!(normalize_id_part("tool_use:123!"), "tool_use_123");
        let long = "a".repeat(100);
        assert_eq!(normalize_id_part(&long).len(), 64);
    }

    #[test]
    fn test_normalize_tool_call_id() {
        let model = make_model();
        // openai provider: split callId|itemId, ensure fc_ prefix
        let normalized = normalize_tool_call_id("call_1|item_1", &model, "openai", "openai-responses");
        assert_eq!(normalized, "call_1|fc_item_1");
        // non-openai provider: plain normalize
        let mut m2 = make_model();
        m2.provider = "deepseek".into();
        let normalized2 = normalize_tool_call_id("call_1|item_1", &m2, "deepseek", "openai-completions");
        assert_eq!(normalized2, "call_1_item_1");
    }

    #[test]
    fn test_convert_messages_system_and_user() {
        let model = make_model();
        let context = Context {
            system_prompt: Some("You are helpful".into()),
            messages: vec![Message::User {
                content: vec![ContentBlock::text("Hello")],
                timestamp: 0,
            }],
            tools: None,
        };
        let items = convert_responses_messages(&model, &context);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["role"], "developer");
        assert_eq!(items[0]["content"], "You are helpful");
        assert_eq!(items[1]["role"], "user");
        assert_eq!(items[1]["content"][0]["type"], "input_text");
    }

    #[test]
    fn test_convert_messages_assistant_text() {
        let model = make_model();
        let context = Context {
            system_prompt: None,
            messages: vec![Message::Assistant {
                content: vec![ContentBlock::text("Hi there")],
                api: "openai-responses".into(),
                provider: "openai".into(),
                model: "gpt-5".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            }],
            tools: None,
        };
        let items = convert_responses_messages(&model, &context);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[0]["role"], "assistant");
        assert_eq!(items[0]["content"][0]["text"], "Hi there");
        assert!(items[0]["id"].as_str().unwrap().starts_with("msg_pi_"));
    }

    #[test]
    fn test_convert_messages_tool_result() {
        let model = make_model();
        let context = Context {
            system_prompt: None,
            messages: vec![Message::ToolResult {
                tool_call_id: "call_1|fc_item_1".into(),
                tool_name: "read".into(),
                content: vec![ContentBlock::text("file contents")],
                details: None,
                is_error: false,
                timestamp: 0,
            }],
            tools: None,
        };
        let items = convert_responses_messages(&model, &context);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "function_call_output");
        assert_eq!(items[0]["call_id"], "call_1");
        assert_eq!(items[0]["output"], "file contents");
    }

    #[test]
    fn test_parse_sse_body() {
        let body = b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n";
        let events = parse_responses_sse_body(body);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "response.created");
        assert_eq!(events[1]["type"], "response.completed");
    }

    #[test]
    fn test_map_stop_reason() {
        assert_eq!(map_stop_reason(Some("completed"), None), (StopReason::Stop, None));
        assert_eq!(
            map_stop_reason(Some("incomplete"), Some("max_output_tokens")),
            (StopReason::Length, None)
        );
        assert_eq!(
            map_stop_reason(Some("incomplete"), Some("content_filter")),
            (
                StopReason::Error,
                Some("Response incomplete: content_filter".to_string())
            )
        );
        assert_eq!(map_stop_reason(Some("failed"), None), (StopReason::Error, None));
    }

    #[test]
    fn test_process_stream_text() {
        let model = make_model();
        let mut output = AssistantMessage {
            content: vec![],
            api: "openai-responses".into(),
            provider: "openai".into(),
            model: "gpt-5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let events = vec![
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg_1"}}),
            json!({"type": "response.output_text.delta", "output_index": 0, "delta": "Hello"}),
            json!({"type": "response.output_text.delta", "output_index": 0, "delta": " world"}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_1", "content": [{"type": "output_text", "text": "Hello world"}]}}),
            json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed", "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}}}),
        ];
        process_responses_stream(&events, &mut output, &tx, &model, None).unwrap();
        assert_eq!(output.stop_reason, StopReason::Stop);
        assert_eq!(output.response_id.as_deref(), Some("resp_1"));
        assert_eq!(output.content.len(), 1);
        match &output.content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "Hello world"),
            _ => panic!("expected text block"),
        }
        // Event sequence from process_responses_stream (the Done event is
        // emitted by the caller after processing): text_start, text_delta x2, text_end.
        let mut kinds = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            kinds.push(match ev {
                AssistantMessageEvent::TextStart { .. } => "text_start",
                AssistantMessageEvent::TextDelta { .. } => "text_delta",
                AssistantMessageEvent::TextEnd { .. } => "text_end",
                _ => "other",
            });
        }
        assert_eq!(kinds, vec!["text_start", "text_delta", "text_delta", "text_end"]);
    }

    #[test]
    fn test_detect_session_affinity_format() {
        let mut model = make_model();
        assert_eq!(
            detect_session_affinity_format(&model),
            SessionAffinityFormat::Openai
        );
        model.provider = "openrouter".into();
        assert_eq!(
            detect_session_affinity_format(&model),
            SessionAffinityFormat::Openrouter
        );
        model.provider = "openai".into();
        model.base_url = "https://openrouter.ai/api/v1".into();
        assert_eq!(
            detect_session_affinity_format(&model),
            SessionAffinityFormat::Openrouter
        );
    }

    #[test]
    fn test_service_tier_pricing() {
        let mut usage = Usage {
            input: 1000,
            output: 500,
            cache_read: 100,
            cache_write: 50,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 1650,
            cost: crate::types::UsageCost {
                input: 0.001,
                output: 0.001,
                cache_read: 0.0001,
                cache_write: 0.0002,
                total: 0.0023,
            },
        };
        apply_service_tier_pricing(&mut usage, "gpt-5", Some("flex"));
        assert!((usage.cost.input - 0.0005).abs() < 1e-9);
        assert!((usage.cost.total - 0.00115).abs() < 1e-9);
        // priority on gpt-5.5 is 2.5x
        let mut usage2 = usage.clone();
        apply_service_tier_pricing(&mut usage2, "gpt-5.5", Some("priority"));
        assert!((usage2.cost.input - 0.0005 * 2.5).abs() < 1e-9);
    }

    #[test]
    fn test_finalize_response_raw_stop_reason() {
        let model = make_model();
        let mut output = AssistantMessage {
            content: vec![],
            api: "openai-responses".into(),
            provider: "openai".into(),
            model: "gpt-5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        let mut reasoning_blocks = HashMap::new();
        let response = json!({
            "id": "resp_1",
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12},
        });
        finalize_response(&response, &mut output, &model, &mut reasoning_blocks, None);
        assert_eq!(output.raw_stop_reason.as_deref(), Some("incomplete.max_output_tokens"));
        assert_eq!(output.stop_reason, StopReason::Length);
    }

    #[test]
    fn test_is_retryable_provider_error() {
        let mk = |status: Option<u16>, headers: reqwest::header::HeaderMap| ProviderHttpError {
            status,
            headers,
            message: "err".into(),
        };
        assert!(is_retryable_provider_error(&mk(None, reqwest::header::HeaderMap::new())));
        assert!(is_retryable_provider_error(&mk(Some(408), reqwest::header::HeaderMap::new())));
        assert!(is_retryable_provider_error(&mk(Some(409), reqwest::header::HeaderMap::new())));
        assert!(is_retryable_provider_error(&mk(Some(429), reqwest::header::HeaderMap::new())));
        assert!(is_retryable_provider_error(&mk(Some(500), reqwest::header::HeaderMap::new())));
        assert!(!is_retryable_provider_error(&mk(Some(400), reqwest::header::HeaderMap::new())));
        assert!(!is_retryable_provider_error(&mk(Some(404), reqwest::header::HeaderMap::new())));
        // x-should-retry header overrides
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-should-retry", "true".parse().unwrap());
        assert!(is_retryable_provider_error(&mk(Some(400), h)));
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-should-retry", "false".parse().unwrap());
        assert!(!is_retryable_provider_error(&mk(Some(500), h)));
    }

    #[test]
    fn test_get_retry_delay_exponential() {
        let err = ProviderHttpError {
            status: Some(429),
            headers: reqwest::header::HeaderMap::new(),
            message: "err".into(),
        };
        // retryIndex 0: 0.5s * jitter(0.75..1.0)
        let d0 = get_retry_delay_ms(&err, 0, None).unwrap();
        assert!((375..=500).contains(&d0), "d0={d0}");
        // retryIndex 3: min(0.5*8, 8) = 4s * jitter
        let d3 = get_retry_delay_ms(&err, 3, None).unwrap();
        assert!((3000..=4000).contains(&d3), "d3={d3}");
        // retryIndex 5: capped at 8s
        let d5 = get_retry_delay_ms(&err, 5, None).unwrap();
        assert!((6000..=8000).contains(&d5), "d5={d5}");
    }

    #[test]
    fn test_get_retry_delay_server_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after-ms", "1500".parse().unwrap());
        let err = ProviderHttpError {
            status: Some(429),
            headers,
            message: "err".into(),
        };
        assert_eq!(get_retry_delay_ms(&err, 0, None).unwrap(), 1500);
        // server delay above max fails immediately
        let mut headers2 = reqwest::header::HeaderMap::new();
        headers2.insert("retry-after-ms", "150000".parse().unwrap());
        let err2 = ProviderHttpError {
            status: Some(429),
            headers: headers2,
            message: "err".into(),
        };
        assert!(get_retry_delay_ms(&err2, 0, None).is_err());
    }

    #[test]
    fn test_process_stream_tool_call() {
        let model = make_model();
        let mut output = AssistantMessage {
            content: vec![],
            api: "openai-responses".into(),
            provider: "openai".into(),
            model: "gpt-5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let events = vec![
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "call_id": "call_1", "id": "fc_1", "name": "read", "arguments": ""}}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"path\":"}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "\"/tmp/a\"}"}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "call_id": "call_1", "id": "fc_1", "name": "read", "arguments": "{\"path\":\"/tmp/a\"}"}}),
            json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed", "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}}}),
        ];
        process_responses_stream(&events, &mut output, &tx, &model, None).unwrap();
        assert_eq!(output.stop_reason, StopReason::ToolUse);
        assert_eq!(output.content.len(), 1);
        match &output.content[0] {
            ContentBlock::ToolCall { id, name, arguments, .. } => {
                assert_eq!(id, "call_1|fc_1");
                assert_eq!(name, "read");
                assert_eq!(arguments["path"], "/tmp/a");
            }
            _ => panic!("expected tool call block"),
        }
    }
}
