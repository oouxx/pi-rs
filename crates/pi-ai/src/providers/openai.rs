//! `OpenAI` Chat Completions API provider.
//!
//! Thin wrapper around the `OpenAI` Chat Completions API using reqwest for HTTP
//! and SSE streaming. Converts between pi-ai types and `OpenAI` API format.
//!
//! Ported from `packages/ai/src/providers/openai-completions.ts`.

use reqwest::Client as HttpClient;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;

use crate::types::{
    AssistantMessage, AssistantMessageEvent, CacheRetention, ContentBlock, Context, Message, Model,
    SimpleStreamOptions, StopReason, StreamOptions, Tool, Usage,
};
use crate::utils::event_stream::AssistantMessageEventStream;

// ============================================================================
// OpenAI-completions compat resolution (match TS `detectCompat` / `getCompat`)
// ============================================================================

#[derive(Debug, Clone)]
pub struct ResolvedOpenAICompletionsCompat {
    pub supports_store: bool,
    pub supports_developer_role: bool,
    pub supports_reasoning_effort: bool,
    pub supports_usage_in_streaming: bool,
    pub supports_finish_reason: bool,
    pub max_tokens_field: String,
    pub requires_tool_result_name: bool,
    pub requires_assistant_after_tool_result: bool,
    pub requires_thinking_as_text: bool,
    pub requires_reasoning_content_on_assistant_messages: bool,
    pub thinking_format: String,
    pub open_router_routing: Option<crate::types::OpenRouterRouting>,
    pub vercel_gateway_routing: Option<crate::types::VercelGatewayRouting>,
    pub chat_template_kwargs: serde_json::Map<String, serde_json::Value>,
    pub chat_template_args: serde_json::Map<String, serde_json::Value>,
    pub zai_tool_stream: bool,
    pub supports_thinking_token_budget: bool,
    pub supports_strict_mode: bool,
    pub supports_openai_grammar_tools: bool,
    pub cache_control_format: Option<String>,
    pub send_session_affinity_headers: bool,
    pub deferred_tools_mode: Option<String>,
    pub session_affinity_format: String,
    pub supports_long_cache_retention: bool,
}

/// Auto-detect compatibility settings from provider name and baseUrl
/// (match TS `detectCompat`). Explicit `model.compat` overrides these.
fn detect_compat(model: &Model) -> ResolvedOpenAICompletionsCompat {
    let provider = &model.provider;
    let base_url = &model.base_url;

    let is_zai = provider == "zai"
        || provider == "zai-coding-cn"
        || base_url.contains("api.z.ai")
        || base_url.contains("open.bigmodel.cn");
    let is_together = provider == "together"
        || base_url.contains("api.together.ai")
        || base_url.contains("api.together.xyz");
    let is_moonshot =
        provider == "moonshotai" || provider == "moonshotai-cn" || base_url.contains("api.moonshot.");
    let is_openrouter = provider == "openrouter" || base_url.contains("openrouter.ai");
    let is_cloudflare_workers_ai =
        provider == "cloudflare-workers-ai" || base_url.contains("api.cloudflare.com");
    let is_cloudflare_ai_gateway =
        provider == "cloudflare-ai-gateway" || base_url.contains("gateway.ai.cloudflare.com");
    let is_nvidia = provider == "nvidia" || base_url.contains("integrate.api.nvidia.com");
    let is_ant_ling = provider == "ant-ling" || base_url.contains("api.ant-ling.com");

    let is_non_standard = is_nvidia
        || provider == "cerebras"
        || base_url.contains("cerebras.ai")
        || provider == "xai"
        || base_url.contains("api.x.ai")
        || is_together
        || base_url.contains("chutes.ai")
        || base_url.contains("deepseek.com")
        || is_zai
        || is_moonshot
        || provider == "opencode"
        || base_url.contains("opencode.ai")
        || is_cloudflare_workers_ai
        || is_cloudflare_ai_gateway
        || is_ant_ling;

    let use_max_tokens = base_url.contains("chutes.ai")
        || is_moonshot
        || is_cloudflare_ai_gateway
        || is_together
        || is_nvidia
        || is_ant_ling
        || is_zai;

    let is_grok = provider == "xai" || base_url.contains("api.x.ai");
    let is_deepseek = provider == "deepseek" || base_url.contains("deepseek.com");
    let is_openrouter_developer_role_model =
        is_openrouter && (model.id.starts_with("anthropic/") || model.id.starts_with("openai/"));
    let cache_control_format = if provider == "openrouter" && model.id.starts_with("anthropic/") {
        Some("anthropic".to_string())
    } else {
        None
    };

    let thinking_format = if is_deepseek {
        "deepseek"
    } else if is_zai {
        "zai"
    } else if is_together {
        "together"
    } else if is_ant_ling {
        "ant-ling"
    } else if is_openrouter {
        "openrouter"
    } else {
        "openai"
    };

    ResolvedOpenAICompletionsCompat {
        supports_store: !is_non_standard,
        supports_developer_role: is_openrouter_developer_role_model || (!is_non_standard && !is_openrouter),
        supports_reasoning_effort: !is_grok
            && !is_zai
            && !is_moonshot
            && !is_together
            && !is_cloudflare_ai_gateway
            && !is_nvidia
            && !is_ant_ling,
        supports_usage_in_streaming: true,
        supports_finish_reason: true,
        max_tokens_field: if use_max_tokens {
            "max_tokens".to_string()
        } else {
            "max_completion_tokens".to_string()
        },
        requires_tool_result_name: false,
        requires_assistant_after_tool_result: false,
        requires_thinking_as_text: false,
        requires_reasoning_content_on_assistant_messages: is_deepseek,
        thinking_format: thinking_format.to_string(),
        open_router_routing: None,
        vercel_gateway_routing: None,
        chat_template_kwargs: serde_json::Map::new(),
        chat_template_args: serde_json::Map::new(),
        zai_tool_stream: false,
        supports_thinking_token_budget: false,
        supports_strict_mode: !is_moonshot && !is_together && !is_cloudflare_ai_gateway && !is_nvidia,
        supports_openai_grammar_tools: false,
        cache_control_format,
        send_session_affinity_headers: false,
        deferred_tools_mode: None,
        session_affinity_format: if is_openrouter {
            "openrouter".to_string()
        } else {
            "openai".to_string()
        },
        supports_long_cache_retention: !(is_together
            || is_cloudflare_workers_ai
            || is_cloudflare_ai_gateway
            || is_nvidia
            || is_ant_ling),
    }
}

/// Get resolved compatibility settings for a model: auto-detect from
/// provider/URL, then override with explicit `model.compat`
/// (match TS `getCompat`).
fn get_compat(model: &Model) -> ResolvedOpenAICompletionsCompat {
    let detected = detect_compat(model);
    let Some(compat) = (match &model.compat {
        Some(crate::types::ModelCompat::OpenAICompletions(c)) => Some(c.as_ref()),
        _ => None,
    }) else {
        return detected;
    };

    ResolvedOpenAICompletionsCompat {
        supports_store: compat.supports_store.unwrap_or(detected.supports_store),
        supports_developer_role: compat
            .supports_developer_role
            .unwrap_or(detected.supports_developer_role),
        supports_reasoning_effort: compat
            .supports_reasoning_effort
            .unwrap_or(detected.supports_reasoning_effort),
        supports_usage_in_streaming: compat
            .supports_usage_in_streaming
            .unwrap_or(detected.supports_usage_in_streaming),
        supports_finish_reason: compat
            .supports_finish_reason
            .unwrap_or(detected.supports_finish_reason),
        max_tokens_field: compat
            .max_tokens_field
            .clone()
            .unwrap_or(detected.max_tokens_field),
        requires_tool_result_name: compat
            .requires_tool_result_name
            .unwrap_or(detected.requires_tool_result_name),
        requires_assistant_after_tool_result: compat
            .requires_assistant_after_tool_result
            .unwrap_or(detected.requires_assistant_after_tool_result),
        requires_thinking_as_text: compat
            .requires_thinking_as_text
            .unwrap_or(detected.requires_thinking_as_text),
        requires_reasoning_content_on_assistant_messages: compat
            .requires_reasoning_content_on_assistant_messages
            .unwrap_or(detected.requires_reasoning_content_on_assistant_messages),
        thinking_format: compat
            .thinking_format
            .clone()
            .unwrap_or(detected.thinking_format),
        open_router_routing: compat.open_router_routing.clone().or(detected.open_router_routing),
        vercel_gateway_routing: compat
            .vercel_gateway_routing
            .clone()
            .or(detected.vercel_gateway_routing),
        chat_template_kwargs: compat
            .chat_template_kwargs
            .clone()
            .unwrap_or(detected.chat_template_kwargs),
        chat_template_args: compat
            .chat_template_args
            .clone()
            .unwrap_or(detected.chat_template_args),
        zai_tool_stream: compat.zai_tool_stream.unwrap_or(detected.zai_tool_stream),
        supports_thinking_token_budget: compat
            .supports_thinking_token_budget
            .unwrap_or(detected.supports_thinking_token_budget),
        supports_strict_mode: compat
            .supports_strict_mode
            .unwrap_or(detected.supports_strict_mode),
        supports_openai_grammar_tools: compat
            .supports_openai_grammar_tools
            .unwrap_or(detected.supports_openai_grammar_tools),
        cache_control_format: compat
            .cache_control_format
            .clone()
            .or(detected.cache_control_format),
        send_session_affinity_headers: compat
            .send_session_affinity_headers
            .unwrap_or(detected.send_session_affinity_headers),
        deferred_tools_mode: compat
            .deferred_tools_mode
            .clone()
            .or(detected.deferred_tools_mode),
        session_affinity_format: compat
            .session_affinity_format
            .clone()
            .unwrap_or(detected.session_affinity_format),
        supports_long_cache_retention: compat
            .supports_long_cache_retention
            .unwrap_or(detected.supports_long_cache_retention),
    }
}

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
// Message conversion (match TS `convertMessages`)
// ============================================================================

/// Convert pi-ai messages to the OpenAI Chat Completions API format.
/// Normalize a tool call id for cross-provider replay into Chat Completions
/// (match TS `normalizeToolCallId` in openai-completions.ts, #6854).
///
/// OpenAI Responses API generates ids that are 450+ chars with special chars
/// like `|` (format `{call_id}|{item_id}`). Multiple tool calls in the same
/// turn can share `call_id` but differ by `item_id`; Chat Completions requires
/// distinct tool call ids, so we preserve item-level uniqueness.
fn normalize_tool_call_id(id: &str, model: &Model) -> String {
    if id.contains('|') {
        let separator_index = id.find('|').unwrap_or(0);
        let call_id: String = id[..separator_index]
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        let item_id: String = id[separator_index + 1..]
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        let combined_id = if item_id.is_empty() {
            call_id.clone()
        } else {
            format!("{call_id}_{item_id}")
        };
        if combined_id.len() <= 40 {
            return combined_id;
        }
        let hash = super::openai_responses::short_hash(id);
        let hash = &hash[..hash.len().min(8)];
        let prefix_len = (40usize.saturating_sub(hash.len() + 1)).max(1);
        let prefix: String = call_id.chars().take(prefix_len).collect();
        return format!("{prefix}_{hash}");
    }

    if model.provider == "openai" {
        return id.chars().take(40).collect();
    }
    id.to_string()
}

fn convert_messages(
    model: &Model,
    context: &Context,
    compat: &ResolvedOpenAICompletionsCompat,
    grammar_properties: &std::collections::HashMap<String, String>,
) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();

    if let Some(sp) = &context.system_prompt {
        let role = if model.reasoning && compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        params.push(json!({ "role": role, "content": sp }));
    }

    let mut last_role: Option<String> = None;

    // Map of original tool call ids to normalized ids (match TS `transformMessages`
    // toolCallIdMap): assistant tool calls are normalized, then tool results are
    // rewritten to the normalized id so they stay linked.
    let mut tool_call_id_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut i = 0usize;
    while i < context.messages.len() {
        let msg = &context.messages[i];
        match msg {
            Message::User { content, .. } => {
                if compat.requires_assistant_after_tool_result
                    && last_role.as_deref() == Some("toolResult")
                {
                    params.push(json!({
                        "role": "assistant",
                        "content": "I have processed the tool results.",
                    }));
                }
                let text: String = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let images: Vec<&ContentBlock> = content
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::Image { .. }))
                    .collect();
                if images.is_empty() {
                    if !text.trim().is_empty() {
                        params.push(json!({ "role": "user", "content": text }));
                    }
                } else {
                    let mut parts: Vec<Value> = Vec::new();
                    if !text.trim().is_empty() {
                        parts.push(json!({ "type": "text", "text": text }));
                    }
                    for b in images {
                        if let ContentBlock::Image { data, mime_type } = b {
                            parts.push(json!({
                                "type": "image_url",
                                "image_url": { "url": format!("data:{mime_type};base64,{data}") },
                            }));
                        }
                    }
                    if !parts.is_empty() {
                        params.push(json!({ "role": "user", "content": parts }));
                    }
                }
                last_role = Some("user".to_string());
            }
            Message::Assistant {
                content,
                api,
                provider,
                model: msg_model,
                ..
            } => {
                // TS `transformMessages` only normalizes tool call ids when the
                // assistant message came from a different model (isSameModel gate).
                let is_same_model = provider == &model.provider
                    && api == &model.api
                    && msg_model == &model.id;
                let text_parts: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                            Some(json!({ "type": "text", "text": text }))
                        }
                        _ => None,
                    })
                    .collect();
                let assistant_text: String = text_parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect();
                let thinking_blocks: Vec<&ContentBlock> = content
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::Thinking { thinking, .. } if !thinking.trim().is_empty()))
                    .collect();

                let mut msg_obj = serde_json::Map::new();
                msg_obj.insert("role".into(), json!("assistant"));
                msg_obj.insert(
                    "content".into(),
                    if compat.requires_assistant_after_tool_result {
                        json!("")
                    } else {
                        Value::Null
                    },
                );

                if !thinking_blocks.is_empty() {
                    if compat.requires_thinking_as_text {
                        let thinking_text: String = thinking_blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let mut parts = vec![json!({ "type": "text", "text": thinking_text })];
                        parts.extend(text_parts);
                        msg_obj.insert("content".into(), json!(parts));
                    } else {
                        // Always send assistant content as a plain string
                        // (OpenAI Chat Completions standard format).
                        if !assistant_text.is_empty() {
                            msg_obj.insert("content".into(), json!(assistant_text));
                        }
                        // Use the signature from the first thinking block if available
                        // (llama.cpp server + gpt-oss).
                        let mut signature = match thinking_blocks[0] {
                            ContentBlock::Thinking {
                                thinking_signature, ..
                            } => thinking_signature.clone(),
                            _ => None,
                        };
                        if model.provider == "opencode-go" && signature.as_deref() == Some("reasoning") {
                            signature = Some("reasoning_content".to_string());
                        }
                        if let Some(sig) = signature {
                            if !sig.is_empty() {
                                let joined: String = thinking_blocks
                                    .iter()
                                    .filter_map(|b| match b {
                                        ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                msg_obj.insert(sig, json!(joined));
                            }
                        }
                    }
                } else if !assistant_text.is_empty() {
                    msg_obj.insert("content".into(), json!(assistant_text));
                }

                let tool_calls: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            let custom_property = grammar_properties.get(name);
                            let normalized_id = if is_same_model {
                                id.clone()
                            } else {
                                normalize_tool_call_id(id, model)
                            };
                            if normalized_id != *id {
                                tool_call_id_map.insert(id.clone(), normalized_id.clone());
                            }
                            if let Some(property) = custom_property {
                                let input = match super::openai_responses::get_grammar_tool_input(
                                    name, arguments, property,
                                ) {
                                    Ok(input) => input,
                                    Err(e) => {
                                        eprintln!("[pi-ai] {e}");
                                        String::new()
                                    }
                                };
                                Some(json!({
                                    "id": normalized_id,
                                    "type": "custom",
                                    "custom": { "name": name, "input": input },
                                }))
                            } else {
                                Some(json!({
                                    "id": normalized_id,
                                    "type": "function",
                                    "function": { "name": name, "arguments": arguments.to_string() },
                                }))
                            }
                        }
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    msg_obj.insert("tool_calls".into(), json!(tool_calls));
                    let reasoning_details: Vec<Value> = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolCall {
                                thought_signature: Some(sig),
                                ..
                            } => serde_json::from_str::<Value>(sig).ok(),
                            _ => None,
                        })
                        .collect();
                    if !reasoning_details.is_empty() {
                        msg_obj.insert("reasoning_details".into(), json!(reasoning_details));
                    }
                }

                if compat.requires_reasoning_content_on_assistant_messages
                    && model.reasoning
                    && !msg_obj.contains_key("reasoning_content")
                {
                    msg_obj.insert("reasoning_content".into(), json!(""));
                }

                let has_content = match msg_obj.get("content") {
                    Some(Value::Null) => false,
                    Some(Value::String(s)) => !s.is_empty(),
                    Some(Value::Array(a)) => !a.is_empty(),
                    _ => false,
                };
                if !has_content && !msg_obj.contains_key("tool_calls") {
                    last_role = Some("assistant".to_string());
                    i += 1;
                    continue;
                }
                params.push(Value::Object(msg_obj));
                last_role = Some("assistant".to_string());
            }
            Message::ToolResult { .. } => {
                // Group consecutive tool results: extract text/images, emit one
                // `tool` message per result, then a single user image message
                // (match TS convertMessages).
                let mut image_blocks: Vec<Value> = Vec::new();
                let mut deferred_names: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut j = i;
                while j < context.messages.len() {
                    if !matches!(context.messages[j], Message::ToolResult { .. }) {
                        break;
                    }
                    let Message::ToolResult {
                        tool_call_id,
                        tool_name,
                        content,
                        added_tool_names,
                        ..
                    } = &context.messages[j]
                    else {
                        break;
                    };
                    let text_result: String = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let has_images = content.iter().any(|b| matches!(b, ContentBlock::Image { .. }));
                    let has_text = !text_result.is_empty();
                    let tool_result_text = if has_text {
                        text_result
                    } else if has_images {
                        "(see attached image)".to_string()
                    } else {
                        "(no tool output)".to_string()
                    };
                    let mut tool_msg = serde_json::Map::new();
                    tool_msg.insert("role".into(), json!("tool"));
                    tool_msg.insert("content".into(), json!(tool_result_text));
                    let effective_id = tool_call_id_map
                        .get(tool_call_id)
                        .cloned()
                        .unwrap_or_else(|| tool_call_id.clone());
                    tool_msg.insert("tool_call_id".into(), json!(effective_id));
                    if compat.requires_tool_result_name {
                        tool_msg.insert("name".into(), json!(tool_name));
                    }
                    params.push(Value::Object(tool_msg));

                    if compat.deferred_tools_mode.as_deref() == Some("kimi") {
                        if let Some(names) = added_tool_names {
                            for name in names {
                                deferred_names.insert(name.clone());
                            }
                        }
                    }

                    if model.input.iter().any(|t| t == "image") {
                        for b in content {
                            if let ContentBlock::Image { data, mime_type } = b {
                                image_blocks.push(json!({
                                    "type": "image_url",
                                    "image_url": { "url": format!("data:{mime_type};base64,{data}") },
                                }));
                            }
                        }
                    }
                    j += 1;
                }
                i = j - 1;

                if !image_blocks.is_empty() {
                    if compat.requires_assistant_after_tool_result {
                        params.push(json!({
                            "role": "assistant",
                            "content": "I have processed the tool results.",
                        }));
                    }
                    let mut parts = vec![json!({
                        "type": "text",
                        "text": "Attached image(s) from tool result:",
                    })];
                    parts.extend(image_blocks);
                    params.push(json!({ "role": "user", "content": parts }));
                    last_role = Some("user".to_string());
                } else {
                    last_role = Some("toolResult".to_string());
                }

                if !deferred_names.is_empty() && compat.deferred_tools_mode.as_deref() == Some("kimi") {
                    let deferred_tools: Vec<Tool> = context
                        .tools
                        .as_ref()
                        .map(|tools| {
                            tools
                                .iter()
                                .filter(|t| deferred_names.contains(&t.name))
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    if !deferred_tools.is_empty() {
                        // Kimi accepts a system message with tools but omits the
                        // standard content field.
                        let mut kimi = serde_json::Map::new();
                        kimi.insert("role".into(), json!("system"));
                        kimi.insert("tools".into(), json!(convert_tools(&deferred_tools, compat)));
                        params.push(Value::Object(kimi));
                    }
                }
            }
        }
        i += 1;
    }

    params
}

/// Convert pi-ai tools to `OpenAI` tool definitions (match TS `convertTools`).
fn convert_tools(tools: &[Tool], compat: &ResolvedOpenAICompletionsCompat) -> Vec<Value> {
    let mut result = Vec::new();
    for tool in tools {
        if let Ok(Some(grammar)) =
            super::openai_responses::resolve_grammar_constrained_sampling(
                tool,
                compat.supports_openai_grammar_tools,
            )
        {
            result.push(json!({
                "type": "custom",
                "custom": {
                    "name": tool.name,
                    "description": tool.description,
                    "format": {
                        "type": "grammar",
                        "grammar": {
                            "syntax": grammar.format,
                            "definition": grammar.definition,
                        },
                    },
                },
            }));
            continue;
        }
        let strict = match super::openai_responses::resolve_json_schema_strict_sampling(
            tool,
            compat.supports_strict_mode,
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[pi-ai] {e}");
                None
            }
        };
        let mut function = serde_json::Map::new();
        function.insert("name".into(), json!(tool.name));
        function.insert("description".into(), json!(tool.description));
        function.insert("parameters".into(), tool.parameters.clone());
        if compat.supports_strict_mode {
            function.insert("strict".into(), json!(strict.unwrap_or(false)));
        }
        result.push(json!({
            "type": "function",
            "function": function,
        }));
    }
    result
}

/// Map `OpenAI` finish reason to pi-ai `StopReason`.
fn map_stop_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        "content_filter" => (
            StopReason::Error,
            Some("Provider finish_reason: content_filter".to_string()),
        ),
        "network_error" => (
            StopReason::Error,
            Some("Provider finish_reason: network_error".to_string()),
        ),
        other => (StopReason::Error, Some(format!("Provider finish_reason: {other}"))),
    }
}

// ============================================================================
// OpenAI SSE parsing (different format from Anthropic)
// ============================================================================

/// Parse a single line from the `OpenAI` SSE stream.
/// `OpenAI` SSE format is simpler: each line is `data: <json>`, ending with `data: [DONE]`.
#[cfg(test)]
fn parse_openai_sse_chunk(line: &str) -> Option<Value> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    // Extract data field. Prefer the `data: ` form (standard SSE) so the
    // remainder carries no leading space; fall back to `data:` for senders
    // that omit the space. Behaviour is identical to the previous if/else
    // chain, expressed with `?` to satisfy clippy::question_mark.
    let data = line
        .strip_prefix("data: ")
        .or_else(|| line.strip_prefix("data:"))?;

    if data == "[DONE]" {
        return None; // end of stream marker
    }

    serde_json::from_str(data).ok()
}

/// Parse a streamed SSE event (already `data:`-decoded) into a chunk value.
fn parse_openai_sse_event(event: &crate::utils::sse::ServerSentEvent) -> Option<Value> {
    if event.data.is_empty() || event.data == "[DONE]" {
        return None;
    }
    serde_json::from_str(&event.data).ok()
}

/// Parse token usage from an `OpenAI` chunk.
///
/// Mirrors `parseChunkUsage` in `packages/ai/src/api/openai-completions.ts`:
/// - `input` excludes cache-read and cache-write tokens (clamped at 0);
/// - cache-read falls back to `prompt_cache_hit_tokens` for OpenRouter-style
///   providers that do not emit `prompt_tokens_details.cached_tokens`;
/// - cache-write comes from `prompt_tokens_details.cache_write_tokens`;
/// - `reasoning` comes from `completion_tokens_details.reasoning_tokens`
///   (OpenAI `completion_tokens` already includes reasoning tokens).
fn parse_chunk_usage(usage: &Value) -> Usage {
    let prompt_tokens = usage
        .get("prompt_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cache_read = usage
        .get("prompt_tokens_details")
        .and_then(|v| v.get("cached_tokens"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            usage
                .get("prompt_cache_hit_tokens")
                .and_then(serde_json::Value::as_u64)
        })
        .unwrap_or(0);
    let cache_write = usage
        .get("prompt_tokens_details")
        .and_then(|v| v.get("cache_write_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .get("completion_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let input = prompt_tokens.saturating_sub(cache_read + cache_write);
    let reasoning = usage
        .get("completion_tokens_details")
        .and_then(|v| v.get("reasoning_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: Some(reasoning),
        total_tokens: input + output + cache_read + cache_write,
        cost: crate::types::UsageCost::default(),
    }
}

// ============================================================================
// StreamOpenAI: main streaming function
// ============================================================================

/// Stream a completion from the `OpenAI` Chat Completions API.
#[must_use]
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
                    raw_stop_reason: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            });
        }
    });

    AssistantMessageEventStream::from_receiver(rx)
}

/// Whether the conversation contains tool calls/results (match TS `hasToolHistory`).
fn has_tool_history(messages: &[Message]) -> bool {
    for msg in messages {
        if matches!(msg, Message::ToolResult { .. }) {
            return true;
        }
        if let Message::Assistant { content, .. } = msg {
            if content.iter().any(|b| matches!(b, ContentBlock::ToolCall { .. })) {
                return true;
            }
        }
    }
    false
}

/// Tool names added by tool results (kimi deferred tools; match TS `getDeferredToolNames`).
fn get_deferred_tool_names(messages: &[Message]) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for msg in messages {
        if let Message::ToolResult {
            usage: None,
            added_tool_names: Some(added),
            ..
        } = msg
        {
            for name in added {
                names.insert(name.clone());
            }
        }
    }
    names
}

fn clamp_openai_prompt_cache_key(key: Option<&str>) -> Option<String> {
    let key = key?;
    let chars: Vec<char> = key.chars().collect();
    if chars.len() > 64 {
        Some(chars[..64].iter().collect())
    } else {
        Some(key.to_string())
    }
}

/// Anthropic-style cache control for openrouter anthropic models
/// (match TS `getCompatCacheControl`).
fn get_compat_cache_control(
    compat: &ResolvedOpenAICompletionsCompat,
    cache_retention: &CacheRetention,
) -> Option<serde_json::Value> {
    if compat.cache_control_format.as_deref() != Some("anthropic")
        || *cache_retention == CacheRetention::None
    {
        return None;
    }
    let ttl = if *cache_retention == CacheRetention::Long && compat.supports_long_cache_retention {
        Some("1h")
    } else {
        None
    };
    Some(match ttl {
        Some(t) => json!({ "type": "ephemeral", "ttl": t }),
        None => json!({ "type": "ephemeral" }),
    })
}

fn add_cache_control_to_message(message: &mut Value, cache_control: &Value) -> bool {
    if let Some(content) = message.get_mut("content") {
        match content {
            Value::String(s) => {
                if s.is_empty() {
                    return false;
                }
                *content = json!([{ "type": "text", "text": s, "cache_control": cache_control }]);
                return true;
            }
            Value::Array(parts) => {
                for part in parts.iter_mut().rev() {
                    if part.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(obj) = part.as_object_mut() {
                            obj.insert("cache_control".into(), cache_control.clone());
                        }
                        return true;
                    }
                }
                return false;
            }
            _ => return false,
        }
    }
    false
}

/// Apply Anthropic-style `cache_control` to the system prompt, the last tool
/// definition, and the last conversation message (match TS `applyAnthropicCacheControl`).
fn apply_anthropic_cache_control(
    messages: &mut [Value],
    tools: &mut Option<Vec<Value>>,
    cache_control: &Value,
) {
    // System/developer prompt (first match wins).
    for message in messages.iter_mut() {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "system" || role == "developer" {
            add_cache_control_to_message(message, cache_control);
            break;
        }
    }
    // Last tool definition.
    if let Some(tools) = tools.as_mut() {
        if let Some(last) = tools.last_mut() {
            if let Some(obj) = last.as_object_mut() {
                obj.insert("cache_control".into(), cache_control.clone());
            }
        }
    }
    // Last user/assistant/tool message.
    for message in messages.iter_mut().rev() {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if (role == "user" || role == "assistant" || role == "tool")
            && add_cache_control_to_message(message, cache_control)
        {
            break;
        }
    }
}

/// Resolve a chat-template kwarg value, supporting `$var`/`omitWhenOff`
/// placeholders (match TS `resolveChatTemplateKwargValue`).
fn resolve_chat_template_kwarg_value(
    model: &Model,
    options: Option<&StreamOptions>,
    value: &Value,
) -> Option<Value> {
    let reasoning_effort = options.and_then(|o| o.reasoning_effort.clone());
    match value {
        Value::Object(obj) => {
            if reasoning_effort.is_none()
                && obj.get("omitWhenOff").and_then(Value::as_bool).unwrap_or(false)
            {
                return None;
            }
            if obj.get("$var").and_then(Value::as_str) == Some("thinking.enabled") {
                return Some(Value::Bool(reasoning_effort.is_some()));
            }
            let mapped = match &reasoning_effort {
                Some(level) => model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|m| m.get(level).and_then(Option::as_deref))
                    .map(str::to_string),
                None => model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|m| m.get("off").and_then(Option::as_deref))
                    .map(str::to_string),
            };
            match mapped {
                Some(v) => Some(Value::String(v)),
                None => reasoning_effort.map(Value::String),
            }
        }
        _ => Some(value.clone()),
    }
}

/// Build `chat_template_kwargs` from compat values (match TS `buildChatTemplateValues`).
fn build_chat_template_values(
    model: &Model,
    options: Option<&StreamOptions>,
    values: &serde_json::Map<String, Value>,
) -> Option<serde_json::Map<String, Value>> {
    let mut resolved = serde_json::Map::new();
    for (key, value) in values {
        if let Some(r) = resolve_chat_template_kwarg_value(model, options, value) {
            resolved.insert(key.clone(), r);
        }
    }
    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

/// Map a reasoning level through the model's `thinkingLevelMap`
/// (match TS `model.thinkingLevelMap?.[level] ?? level` semantics).
fn resolve_effort(model: &Model, level: &str) -> Option<String> {
    match model.thinking_level_map.as_ref().and_then(|m| m.get(level)) {
        Some(Some(v)) => Some(v.clone()),
        // Explicitly disabled (null) → no effort field.
        Some(None) => None,
        None => Some(level.to_string()),
    }
}

/// Apply provider-specific thinking/reasoning request params
/// (match TS `buildParams` thinkingFormat branches).
fn apply_thinking_params(
    model: &Model,
    options: Option<&StreamOptions>,
    compat: &ResolvedOpenAICompletionsCompat,
    body: &mut serde_json::Map<String, Value>,
) {
    let reasoning_effort = options.and_then(|o| o.reasoning_effort.clone());
    let supports_reasoning_effort = compat.supports_reasoning_effort;
    match compat.thinking_format.as_str() {
        "zai" if model.reasoning => {
            body.insert(
                "thinking".into(),
                if reasoning_effort.is_some() {
                    json!({ "type": "enabled", "clear_thinking": false })
                } else {
                    json!({ "type": "disabled" })
                },
            );
            if let Some(level) = reasoning_effort.as_deref() {
                if supports_reasoning_effort {
                    if let Some(e) = resolve_effort(model, level) {
                        body.insert("reasoning_effort".into(), Value::String(e));
                    }
                }
            }
        }
        "qwen" if model.reasoning => {
            body.insert(
                "enable_thinking".into(),
                Value::Bool(reasoning_effort.is_some()),
            );
            if let Some(level) = reasoning_effort.as_deref() {
                if supports_reasoning_effort {
                    if let Some(e) = resolve_effort(model, level) {
                        body.insert("reasoning_effort".into(), Value::String(e));
                    }
                }
            }
        }
        "qwen-chat-template" if model.reasoning => {
            body.insert(
                "chat_template_kwargs".into(),
                json!({
                    "enable_thinking": reasoning_effort.is_some(),
                    "preserve_thinking": true,
                }),
            );
        }
        "chat-template" if model.reasoning => {
            if let Some(kwargs) = build_chat_template_values(model, options, &compat.chat_template_kwargs) {
                body.insert("chat_template_kwargs".into(), Value::Object(kwargs));
            }
        }
        "baseten" if model.reasoning => {
            if let Some(args) = build_chat_template_values(model, options, &compat.chat_template_args) {
                body.insert("chat_template_args".into(), Value::Object(args));
            }
            if supports_reasoning_effort {
                let requested = reasoning_effort.as_deref();
                let effort = match requested {
                    Some(level) => resolve_effort(model, level),
                    None => model
                        .thinking_level_map
                        .as_ref()
                        .and_then(|m| m.get("off").and_then(Option::as_deref))
                        .map(str::to_string),
                };
                if let Some(e) = effort {
                    body.insert("reasoning_effort".into(), Value::String(e));
                }
            }
        }
        "deepseek" if model.reasoning => {
            if reasoning_effort.is_some() {
                body.insert("thinking".into(), json!({ "type": "enabled" }));
            } else if model
                .thinking_level_map
                .as_ref()
                .and_then(|m| m.get("off").and_then(Option::as_deref))
                .is_some()
            {
                body.insert("thinking".into(), json!({ "type": "disabled" }));
            }
            if let Some(level) = reasoning_effort.as_deref() {
                if supports_reasoning_effort {
                    let effort = resolve_effort(model, level).unwrap_or_else(|| level.to_string());
                    body.insert("reasoning_effort".into(), Value::String(effort));
                }
            }
        }
        "openrouter" if model.reasoning => {
            if reasoning_effort.is_some() {
                let effort = resolve_effort(model, reasoning_effort.as_deref().unwrap_or(""))
                    .unwrap_or_else(|| reasoning_effort.clone().unwrap_or_default());
                body.insert("reasoning".into(), json!({ "effort": effort }));
            } else if model.thinking_level_map.as_ref().and_then(|m| m.get("off").and_then(Option::as_deref)).is_some() {
                let off = model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|m| m.get("off").and_then(Option::as_deref))
                    .map(str::to_string)
                    .unwrap_or_else(|| "none".to_string());
                body.insert("reasoning".into(), json!({ "effort": off }));
            }
        }
        "ant-ling" if model.reasoning && reasoning_effort.is_some() => {
            if let Some(e) = resolve_effort(model, reasoning_effort.as_deref().unwrap_or("")) {
                body.insert("reasoning".into(), json!({ "effort": e }));
            }
        }
        "together" if model.reasoning => {
            body.insert(
                "reasoning".into(),
                json!({ "enabled": reasoning_effort.is_some() }),
            );
            if let Some(level) = reasoning_effort.as_deref() {
                if supports_reasoning_effort {
                    let effort = resolve_effort(model, level).unwrap_or_else(|| level.to_string());
                    body.insert("reasoning_effort".into(), Value::String(effort));
                }
            }
        }
        "string-thinking" if model.reasoning => {
            if reasoning_effort.is_some() {
                let level = reasoning_effort.as_deref().unwrap_or("");
                let effort = resolve_effort(model, level).unwrap_or_else(|| level.to_string());
                body.insert("thinking".into(), Value::String(effort));
            } else if model.thinking_level_map.as_ref().and_then(|m| m.get("off").and_then(Option::as_deref)).is_some() {
                let off = model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|m| m.get("off").and_then(Option::as_deref))
                    .map(str::to_string)
                    .unwrap_or_else(|| "none".to_string());
                body.insert("thinking".into(), Value::String(off));
            }
        }
        _ => {
            // OpenAI-style reasoning_effort.
            if let Some(level) = reasoning_effort.as_deref() {
                if model.reasoning && supports_reasoning_effort {
                    if let Some(e) = resolve_effort(model, level) {
                        body.insert("reasoning_effort".into(), Value::String(e));
                    }
                }
            } else if model.reasoning && supports_reasoning_effort {
                if let Some(off) = model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|m| m.get("off").and_then(Option::as_deref))
                {
                    body.insert("reasoning_effort".into(), Value::String(off.to_string()));
                }
            }
        }
    }

    // vLLM caps reasoning with a top-level thinking_token_budget
    // (match TS buildParams).
    if compat.supports_thinking_token_budget
        && reasoning_effort.is_some()
        && model.reasoning
    {
        let level = if reasoning_effort.as_deref() == Some("xhigh")
            || reasoning_effort.as_deref() == Some("max")
        {
            "high"
        } else {
            reasoning_effort.as_deref().unwrap_or("")
        };
        let budget_value = |default: u64| -> Option<u64> {
            options
                .and_then(|o| o.thinking_budgets.as_ref())
                .and_then(|b| match level {
                    "minimal" => b.minimal,
                    "low" => b.low,
                    "medium" => b.medium,
                    "high" => b.high,
                    _ => None,
                })
                .or(Some(default))
        };
        let budget_floor = match level {
            "minimal" => budget_value(1024),
            "low" => budget_value(2048),
            "medium" => budget_value(8192),
            "high" => budget_value(16384),
            _ => None,
        };
        let ceiling = body
            .get("max_tokens")
            .and_then(Value::as_u64)
            .or_else(|| body.get("max_completion_tokens").and_then(Value::as_u64))
            .or(Some(model.max_tokens));
        if let (Some(floor), Some(ceiling)) = (budget_floor, ceiling) {
            let budget = floor.min(ceiling.saturating_sub(1024));
            if budget > 0 {
                body.insert("thinking_token_budget".into(), Value::Number(budget.into()));
            }
        }
    }
}

async fn stream_openai_inner(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
    api_key: Option<&str>,
    tx: &tokio::sync::mpsc::UnboundedSender<AssistantMessageEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // api_key 为 None / 空字符串时，不发 Authorization header。
    // 本地 provider（如 Ollama）不需要鉴权；需要鉴权的 provider 没带 key
    // 时，缺少 header 会被服务端以 401 拒绝，行为与发空 Bearer 一致。
    let api_key = api_key.filter(|k| !k.is_empty());
    let max_tokens = options.and_then(|o| o.max_tokens);
    let temperature = options.and_then(|o| o.temperature);
    let signal = options.and_then(|o| o.signal.clone());

    let http_client = HttpClient::new();
    let compat = get_compat(model);

    let grammar_properties = super::openai_responses::grammar_tool_input_properties(
        context,
        compat.supports_openai_grammar_tools,
    );
    let mut messages = convert_messages(model, context, &compat, &grammar_properties);

    // Cache retention (match TS `resolveCacheRetention`).
    let cache_retention = options
        .and_then(|o| o.cache_retention.clone())
        .unwrap_or_else(|| {
            if std::env::var("PI_CACHE_RETENTION").as_deref() == Ok("long") {
                CacheRetention::Long
            } else {
                CacheRetention::Short
            }
        });
    let prompt_cache_key = if (model.base_url.contains("api.openai.com")
        && cache_retention != CacheRetention::None)
        || (cache_retention == CacheRetention::Long && compat.supports_long_cache_retention)
    {
        clamp_openai_prompt_cache_key(options.and_then(|o| o.session_id.as_deref()))
    } else {
        None
    };
    let prompt_cache_retention = if cache_retention == CacheRetention::Long
        && compat.supports_long_cache_retention
    {
        Some("24h")
    } else {
        None
    };

    // Tools: kimi deferred tools are excluded from the active tool set; some
    // providers require a tools param when the conversation has tool history.
    let deferred_tool_names = if compat.deferred_tools_mode.as_deref() == Some("kimi") {
        get_deferred_tool_names(&context.messages)
    } else {
        std::collections::HashSet::new()
    };
    let active_tools: Vec<Tool> = context
        .tools
        .as_ref()
        .map(|tools| {
            tools
                .iter()
                .filter(|t| !deferred_tool_names.contains(&t.name))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut tools = if active_tools.is_empty() {
        if has_tool_history(&context.messages) {
            Some(Vec::new())
        } else {
            None
        }
    } else {
        Some(convert_tools(&active_tools, &compat))
    };

    // Anthropic-style cache control on messages/tools.
    if let Some(cache_control) = get_compat_cache_control(&compat, &cache_retention) {
        apply_anthropic_cache_control(&mut messages, &mut tools, &cache_control);
    }

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(model.id.clone()));
    body.insert("messages".to_string(), serde_json::to_value(&messages)?);
    body.insert("stream".to_string(), Value::Bool(true));
    if let Some(key) = prompt_cache_key {
        body.insert("prompt_cache_key".to_string(), Value::String(key));
    }
    if let Some(ret) = prompt_cache_retention {
        body.insert("prompt_cache_retention".to_string(), Value::String(ret.to_string()));
    }
    if compat.supports_usage_in_streaming {
        body.insert(
            "stream_options".to_string(),
            serde_json::json!({"include_usage": true}),
        );
    }
    if compat.supports_store {
        body.insert("store".to_string(), Value::Bool(false));
    }

    if let Some(mt) = max_tokens {
        body.insert(compat.max_tokens_field.clone(), Value::Number(mt.into()));
    }
    if let Some(t) = temperature {
        body.insert(
            "temperature".to_string(),
            Value::Number(
                serde_json::Number::from_f64(t).unwrap_or_else(|| serde_json::Number::from(1)),
            ),
        );
    }
    if let Some(ref t) = tools {
        body.insert("tools".to_string(), serde_json::to_value(t)?);
        if compat.zai_tool_stream {
            body.insert("tool_stream".to_string(), Value::Bool(true));
        }
    }
    if let Some(ref tc) = options.and_then(|o| o.tool_choice.as_ref()) {
        body.insert("tool_choice".to_string(), serde_json::to_value(tc)?);
    }

    apply_thinking_params(model, options, &compat, &mut body);

    // OpenRouter provider routing preferences.
    if let Some(routing) = &compat.open_router_routing {
        body.insert("provider".to_string(), serde_json::to_value(routing)?);
    }

    // Check abort signal
    if let Some(ref rx) = signal {
        if *rx.borrow() {
            return Err("Request was aborted".into());
        }
    }

    let request_body = Value::Object(body);
    let mut headers: Vec<(String, String)> = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    if let Some(key) = api_key {
        headers.push(("Authorization".to_string(), format!("Bearer {key}")));
    }
    // Session-affinity headers (match TS createClient).
    if let Some(session_id) = options.and_then(|o| o.session_id.clone()) {
        if compat.send_session_affinity_headers {
            if compat.session_affinity_format == "openrouter" {
                headers.push(("x-session-id".to_string(), session_id));
            } else {
                if compat.session_affinity_format == "openai" {
                    headers.push(("session_id".to_string(), session_id.clone()));
                }
                headers.push(("x-client-request-id".to_string(), session_id.clone()));
                headers.push(("x-session-affinity".to_string(), session_id));
            }
        }
    }
    let mut request = http_client
        .post(format!(
            "{}/chat/completions",
            model.base_url.trim_end_matches('/')
        ))
        .json(&request_body);
    for (k, v) in &headers {
        request = request.header(k, v);
    }
    let response = request.send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("OpenAI API error {status}: {text}").into());
    }

    // Stream the SSE body incrementally (match TS openai-completions streaming).
    let events = crate::utils::sse::sse_events_stream(
        response.bytes_stream().map(|item| item.map(|b| b.to_vec())),
    );
    futures::pin_mut!(events);

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
        raw_stop_reason: None,
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

    use futures::StreamExt;

    // Abort future: completes when the abort signal flips to true. When there
    // is no signal, it never completes (matching TS, where the fetch is passed
    // the AbortSignal and aborts actively — here we select on the signal so an
    // abort interrupts the SSE read immediately instead of waiting for the
    // next chunk).
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
            event = events.next() => event,
            _ = &mut abort_fut => {
                // Abort requested — mark the message as aborted (matching TS
                // `output.stopReason = signal.aborted ? "aborted" : "error"`)
                // so downstream compaction/retry checks skip it.
                output.stop_reason = StopReason::Aborted;
                output.error_message = Some("Request was aborted".to_string());
                let _ = tx.send(AssistantMessageEvent::Error {
                    reason: StopReason::Aborted,
                    error: output.clone(),
                });
                return Ok(());
            }
        };
        let Some(event) = next else {
            break;
        };
        let event = event?;
        let Some(chunk) = parse_openai_sse_event(&event) else {
            continue;
        };
        // (Abort is handled by the select above; no per-chunk poll needed.)

        // Capture response ID and model (routed model surfaces on responseModel
        // when it differs from the requested id — match TS openai-completions).
        if output.response_id.is_none() {
            if let Some(id) = chunk.get("id").and_then(|v| v.as_str()) {
                output.response_id = Some(id.to_string());
            }
        }
        if output.response_model.is_none() {
            if let Some(m) = chunk.get("model").and_then(|v| v.as_str()) {
                if !m.is_empty() && m != model.id {
                    output.response_model = Some(m.to_string());
                }
            }
        }

        // Parse usage if present
        if let Some(usage) = chunk.get("usage") {
            output.usage = parse_chunk_usage(usage);
        }

        // Parse choices
        let Some(choices) = chunk.get("choices").and_then(|v| v.as_array()) else {
            continue;
        };

        if choices.is_empty() {
            continue;
        }
        let choice = &choices[0];

        // Check finish reason
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            if !reason.is_empty() {
                let (stop_reason, error_message) = map_stop_reason(reason);
                output.stop_reason = stop_reason;
                output.raw_stop_reason = Some(reason.to_string());
                if let Some(msg) = error_message {
                    output.error_message = Some(msg);
                }
                has_finish_reason = true;
            }
        }

        // Parse delta
        let Some(delta) = choice.get("delta") else {
            continue;
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
                        t.clone_from(text);
                    }
                    let _ = tx.send(AssistantMessageEvent::TextDelta {
                        content_index: ci,
                        delta: content.to_string(),
                        partial: output.clone(),
                    });
                }
            }
        }

        // Reasoning/thinking content. Some endpoints return reasoning in
        // reasoning_content (llama.cpp), or reasoning (other OpenAI-compatible
        // endpoints). Use the FIRST non-empty reasoning field to avoid
        // duplication (e.g. chutes.ai returns both reasoning_content and
        // reasoning with the same content) — match TS openai-completions.
        let reasoning_field = ["reasoning_content", "reasoning", "reasoning_text"]
            .iter()
            .find(|f| delta.get(**f).and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false))
            .copied();
        if let Some(field) = reasoning_field {
            let reasoning = delta.get(field).and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !reasoning.is_empty() {
                // opencode-go maps the `reasoning` field to a
                // `reasoning_content` signature (match TS).
                let thinking_signature = if model.provider == "opencode-go" && field == "reasoning" {
                    "reasoning_content"
                } else {
                    field
                };
                // For simplicity, treat reasoning as thinking blocks
                // Find or create a thinking block
                let thinking_idx = output
                    .content
                    .iter()
                    .position(|b| matches!(b, ContentBlock::Thinking { .. }));
                if let Some(ti) = thinking_idx {
                    if let Some(ContentBlock::Thinking {
                        thinking: ref mut t, ..
                    }) = output.content.get_mut(ti)
                    {
                        t.push_str(&reasoning);
                        let _ = tx.send(AssistantMessageEvent::ThinkingDelta {
                            content_index: ti,
                            delta: reasoning.clone(),
                            partial: output.clone(),
                        });
                    }
                } else {
                    let ci = output.content.len();
                    output.content.push(ContentBlock::Thinking {
                        thinking: reasoning.clone(),
                        thinking_signature: Some(thinking_signature.to_string()),
                        redacted: None,
                    });
                    let _ = tx.send(AssistantMessageEvent::ThinkingStart {
                        content_index: ci,
                        partial: output.clone(),
                    });
                }
            }
        }

        // Tool calls — using dual-map lookup (by index, by id) aligned with TS pi
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tc in tool_calls {
                let stream_index = tc
                    .get("index")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize;
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
                        .unwrap_or_else(|_| Value::Object(serde_json::Map::default()));

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

    // Match TS: check that we received a finish_reason. If the stream ended
    // but an abort was requested (e.g. the provider closed the connection
    // right as the user cancelled), mark the message as aborted — matching TS
    // `output.stopReason = signal.aborted ? "aborted" : "error"` — so
    // downstream compaction/retry checks skip it.
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

/// Stream a completion from `OpenAI` with simplified options.
#[must_use]
pub fn stream_simple_openai(
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
        full_opts
            .cache_retention
            .clone_from(&opts.base.cache_retention);
        full_opts.session_id.clone_from(&opts.base.session_id);
        full_opts.headers.clone_from(&opts.base.headers);
        full_opts.timeout_ms = opts.base.timeout_ms;
        full_opts.max_retries = opts.base.max_retries;
        full_opts.max_retry_delay_ms = opts.base.max_retry_delay_ms;
        full_opts.metadata.clone_from(&opts.base.metadata);
        full_opts.reasoning_effort.clone_from(&opts.reasoning);
        full_opts.thinking_budgets.clone_from(&opts.thinking_budgets);
    }
    stream_openai(model, context, Some(&full_opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    // ============================================================
    // map_stop_reason tests
    // ============================================================

    fn test_model(provider: &str) -> Model {
        Model {
            id: "test-model".to_string(),
            name: "Test Model".to_string(),
            api: "openai-completions".to_string(),
            provider: provider.to_string(),
            base_url: "https://example.com".to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: crate::types::ModelCost::default(),
            context_window: 128_000,
            max_tokens: 4096,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn test_normalize_tool_call_id_pipe() {
        // Pipe-separated ids from Responses API: {call_id}|{item_id} (TS #6854)
        let model = test_model("github-copilot");
        let id = "call_abc|item_xyz";
        let normalized = normalize_tool_call_id(id, &model);
        assert_eq!(normalized, "call_abc_item_xyz");
    }

    #[test]
    fn test_normalize_tool_call_id_pipe_sanitizes() {
        // Special chars in the item id are replaced with underscores
        let model = test_model("opencode");
        let id = "call_1|item+with/special=chars";
        let normalized = normalize_tool_call_id(id, &model);
        assert_eq!(normalized, "call_1_item_with_special_chars");
    }

    #[test]
    fn test_normalize_tool_call_id_pipe_long_hashes() {
        // Combined id over 40 chars: prefix + 8-char hash
        let model = test_model("opencode");
        let id = "call_very_long_call_id_1234567890|item_very_long_item_id_abcdefghijklmnopqrstuvwxyz";
        let normalized = normalize_tool_call_id(id, &model);
        assert!(normalized.len() <= 40, "normalized id too long: {normalized}");
        // prefix = call_id truncated to 40 - 8 (hash) - 1 (separator) = 31 chars
        assert!(normalized.starts_with("call_very_long_call_id_12345678_"));
    }

    #[test]
    fn test_normalize_tool_call_id_openai_truncates() {
        // openai provider: truncate to 40 chars
        let model = test_model("openai");
        let long_id = "a".repeat(100);
        let normalized = normalize_tool_call_id(&long_id, &model);
        assert_eq!(normalized.len(), 40);
    }

    #[test]
    fn test_normalize_tool_call_id_plain_passthrough() {
        // Non-openai, no pipe: unchanged
        let model = test_model("anthropic");
        let id = "toolu_01ABC";
        assert_eq!(normalize_tool_call_id(id, &model), id);
    }

    #[test]
    fn test_map_stop_reason_stop() {
        assert_eq!(map_stop_reason("stop"), (StopReason::Stop, None));
        assert_eq!(map_stop_reason("end"), (StopReason::Stop, None));
    }

    #[test]
    fn test_map_stop_reason_length() {
        assert_eq!(map_stop_reason("length"), (StopReason::Length, None));
    }

    #[test]
    fn test_map_stop_reason_tool_calls() {
        assert_eq!(map_stop_reason("tool_calls"), (StopReason::ToolUse, None));
        assert_eq!(map_stop_reason("function_call"), (StopReason::ToolUse, None));
    }

    #[test]
    fn test_map_stop_reason_content_filter() {
        assert_eq!(
            map_stop_reason("content_filter"),
            (
                StopReason::Error,
                Some("Provider finish_reason: content_filter".to_string())
            )
        );
    }

    #[test]
    fn test_map_stop_reason_network_error() {
        assert_eq!(
            map_stop_reason("network_error"),
            (
                StopReason::Error,
                Some("Provider finish_reason: network_error".to_string())
            )
        );
    }

    #[test]
    fn test_map_stop_reason_unknown() {
        assert_eq!(
            map_stop_reason("unknown"),
            (
                StopReason::Error,
                Some("Provider finish_reason: unknown".to_string())
            )
        );
    }

    // ============================================================
    // convert_messages tests
    // ============================================================

    fn make_test_model() -> Model {
        Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: "openai-completions".into(),
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
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
    fn test_convert_messages_user() {
        let model = make_test_model();
        let compat = get_compat(&model);
        let context = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![ContentBlock::text("Hello")],
                timestamp: 1000,
            }],
            tools: None,
        };
        let converted = convert_messages(&model, &context, &compat, &std::collections::HashMap::new());
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "user");
        assert_eq!(converted[0]["content"], "Hello");
    }

    #[test]
    fn test_convert_messages_system_prompt_uses_developer_role_for_reasoning() {
        let mut model = make_test_model();
        model.reasoning = true;
        let compat = get_compat(&model);
        let context = Context {
            system_prompt: Some("You are helpful".into()),
            messages: vec![Message::User {
                content: vec![ContentBlock::text("Hi")],
                timestamp: 1000,
            }],
            tools: None,
        };
        let converted = convert_messages(&model, &context, &compat, &std::collections::HashMap::new());
        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["role"], "developer");
        assert_eq!(converted[0]["content"], "You are helpful");
    }

    #[test]
    fn test_convert_messages_assistant_with_tool_calls() {
        let model = make_test_model();
        let compat = get_compat(&model);
        let context = Context {
            system_prompt: None,
            messages: vec![Message::Assistant {
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
            }],
            tools: None,
        };
        let converted = convert_messages(&model, &context, &compat, &std::collections::HashMap::new());
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "assistant");
        let tcs = converted[0]["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["function"]["name"], "get_weather");
        assert_eq!(tcs[0]["function"]["arguments"], "{\"city\":\"NYC\"}");
    }

    #[test]
    fn test_convert_messages_thinking_uses_signature_field() {
        let mut model = make_test_model();
        model.provider = "deepseek".into();
        let compat = get_compat(&model);
        assert!(compat.requires_reasoning_content_on_assistant_messages);
        let context = Context {
            system_prompt: None,
            messages: vec![Message::Assistant {
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "reasoning text".into(),
                        thinking_signature: Some("reasoning_content".into()),
                        redacted: None,
                    },
                    ContentBlock::text("answer"),
                ],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-r1".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 1000,
            }],
            tools: None,
        };
        let converted = convert_messages(&model, &context, &compat, &std::collections::HashMap::new());
        assert_eq!(converted[0]["role"], "assistant");
        assert_eq!(converted[0]["content"], "answer");
        assert_eq!(converted[0]["reasoning_content"], "reasoning text");
    }

    #[test]
    fn test_convert_messages_tool_result() {
        let model = make_test_model();
        let compat = get_compat(&model);
        let context = Context {
            system_prompt: None,
            messages: vec![Message::ToolResult {
                tool_call_id: "call_1".into(),
                tool_name: "get_weather".into(),
                content: vec![ContentBlock::text("72F sunny")],
                details: None,
                is_error: false,
                usage: None,
                added_tool_names: None,
                timestamp: 1000,
            }],
            tools: None,
        };
        let converted = convert_messages(&model, &context, &compat, &std::collections::HashMap::new());
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "tool");
        assert_eq!(converted[0]["tool_call_id"], "call_1");
    }

    #[test]
    fn test_convert_messages_empty_user() {
        let model = make_test_model();
        let compat = get_compat(&model);
        let context = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![ContentBlock::text("")],
                timestamp: 1000,
            }],
            tools: None,
        };
        let converted = convert_messages(&model, &context, &compat, &std::collections::HashMap::new());
        assert_eq!(converted.len(), 0);
    }

    #[test]
    fn test_detect_compat_max_tokens_field() {
        // Together/baseUrl detection drives max_tokens vs max_completion_tokens.
        let mut model = make_test_model();
        model.provider = "together".into();
        let compat = get_compat(&model);
        assert_eq!(compat.max_tokens_field, "max_tokens");
        model.provider = "openai".into();
        model.base_url = "https://api.openai.com/v1".into();
        let compat = get_compat(&model);
        assert_eq!(compat.max_tokens_field, "max_completion_tokens");
    }

    #[test]
    fn test_detect_compat_openrouter_session_affinity() {
        let mut model = make_test_model();
        model.provider = "openrouter".into();
        model.base_url = "https://openrouter.ai/api/v1".into();
        let compat = get_compat(&model);
        assert_eq!(compat.session_affinity_format, "openrouter");
        // Anthropic models via openrouter use anthropic cache-control format.
        model.id = "anthropic/claude-3.5-sonnet".into();
        let compat = get_compat(&model);
        assert_eq!(compat.cache_control_format.as_deref(), Some("anthropic"));
    }

    #[test]
    fn test_get_compat_explicit_overrides_detected() {
        let mut model = make_test_model();
        model.provider = "together".into();
        model.compat = Some(crate::types::ModelCompat::OpenAICompletions(Box::new(
            crate::types::OpenAICompletionsCompat {
                max_tokens_field: Some("max_completion_tokens".into()),
                ..Default::default()
            },
        )));
        let compat = get_compat(&model);
        assert_eq!(compat.max_tokens_field, "max_completion_tokens");
        // Other detected values still apply.
        assert_eq!(compat.session_affinity_format, "openai");
    }

    // ============================================================
    // convert_tools tests
    // ============================================================

    #[test]
    fn test_convert_tools() {
        let model = make_test_model();
        let compat = get_compat(&model);
        let tools = vec![Tool {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            constrained_sampling: None,
        }];
        let converted = convert_tools(&tools, &compat);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["type"], "function");
        assert_eq!(converted[0]["function"]["name"], "read");
    }

    // ============================================================
    // chat-template kwargs / thinking params tests (#12)
    // ============================================================

    #[test]
    fn test_chat_template_kwargs_thinking() {
        // vLLM/HF chat-template models (#12): `chat_template_kwargs` with a
        // `$var: "thinking.enabled"` placeholder resolves to the reasoning
        // effort presence, and `omitWhenOff` drops the kwarg when off.
        let model = make_test_model();
        let kwargs = serde_json::json!({
            "enable_thinking": { "$var": "thinking.enabled" },
            "preserve_thinking": true,
            "effort_hint": { "omitWhenOff": true },
        });
        let values = kwargs.as_object().unwrap().clone();

        // reasoning effort present → enable_thinking=true, effort_hint included.
        let opts = StreamOptions {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        };
        let resolved = build_chat_template_values(&model, Some(&opts), &values).unwrap();
        assert_eq!(resolved["enable_thinking"], true);
        assert_eq!(resolved["preserve_thinking"], true);
        assert!(resolved.contains_key("effort_hint"));

        // reasoning effort off → enable_thinking=false, effort_hint omitted.
        let opts = StreamOptions::default();
        let resolved = build_chat_template_values(&model, Some(&opts), &values).unwrap();
        assert_eq!(resolved["enable_thinking"], false);
        assert!(!resolved.contains_key("effort_hint"));
    }

    #[test]
    fn test_thinking_params_chat_template_format() {
        // compat.thinkingFormat="chat-template" emits chat_template_kwargs.
        let mut model = make_test_model();
        model.reasoning = true;
        model.compat = Some(crate::types::ModelCompat::OpenAICompletions(Box::new(
            crate::types::OpenAICompletionsCompat {
                thinking_format: Some("chat-template".into()),
                chat_template_kwargs: Some({
                    let mut m = serde_json::Map::new();
                    m.insert(
                        "enable_thinking".into(),
                        serde_json::json!({ "$var": "thinking.enabled" }),
                    );
                    m
                }),
                ..Default::default()
            },
        )));
        let compat = get_compat(&model);
        let mut body = serde_json::Map::new();
        let opts = StreamOptions {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        };
        apply_thinking_params(&model, Some(&opts), &compat, &mut body);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
    }

    #[test]
    fn test_thinking_params_openai_reasoning_effort() {
        // Default "openai" format emits reasoning_effort, mapped through the
        // model's thinking level map.
        let mut model = make_test_model();
        model.reasoning = true;
        let mut map = std::collections::HashMap::new();
        map.insert("high".to_string(), Some("reasoning_high".to_string()));
        map.insert("off".to_string(), Some("none".to_string()));
        model.thinking_level_map = Some(map);
        let compat = get_compat(&model);
        let mut body = serde_json::Map::new();
        let opts = StreamOptions {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        };
        apply_thinking_params(&model, Some(&opts), &compat, &mut body);
        assert_eq!(body["reasoning_effort"], "reasoning_high");

        // Off (no effort) emits the mapped "off" value.
        let mut body = serde_json::Map::new();
        apply_thinking_params(&model, None, &compat, &mut body);
        assert_eq!(body["reasoning_effort"], "none");
    }

    #[test]
    fn test_thinking_params_zai_format() {
        let mut model = make_test_model();
        model.reasoning = true;
        model.provider = "zai".into();
        let compat = get_compat(&model);
        assert_eq!(compat.thinking_format, "zai");
        let mut body = serde_json::Map::new();
        let opts = StreamOptions {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        };
        apply_thinking_params(&model, Some(&opts), &compat, &mut body);
        assert_eq!(body["thinking"]["type"], "enabled");
        // zai doesn't support reasoning_effort per detectCompat.
        assert!(!body.contains_key("reasoning_effort"));
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
        // input excludes cache-read tokens (mirrors TS parseChunkUsage)
        assert_eq!(usage.input, 80);
        assert_eq!(usage.output, 50);
        assert_eq!(usage.total_tokens, 150);
        assert_eq!(usage.cache_read, 20);
        assert_eq!(usage.cache_write, 0);
        assert_eq!(usage.reasoning, Some(0));
    }

    #[test]
    fn test_parse_chunk_usage_cache_write_and_reasoning() {
        let usage_json = serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "prompt_tokens_details": {
                "cached_tokens": 20,
                "cache_write_tokens": 10
            },
            "completion_tokens_details": {"reasoning_tokens": 30}
        });
        let usage = parse_chunk_usage(&usage_json);
        assert_eq!(usage.input, 70);
        assert_eq!(usage.cache_read, 20);
        assert_eq!(usage.cache_write, 10);
        assert_eq!(usage.reasoning, Some(30));
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_parse_chunk_usage_prompt_cache_hit_fallback() {
        // OpenRouter-style providers emit prompt_cache_hit_tokens instead of
        // prompt_tokens_details.cached_tokens.
        let usage_json = serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "prompt_cache_hit_tokens": 20
        });
        let usage = parse_chunk_usage(&usage_json);
        assert_eq!(usage.input, 80);
        assert_eq!(usage.cache_read, 20);
    }
}

#[cfg(test)]
mod abort_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use futures::StreamExt;

    fn abort_test_model(addr: &str) -> Model {
        Model {
            id: "test-model".to_string(),
            name: "Test Model".to_string(),
            api: "openai-completions".to_string(),
            provider: "ollama".to_string(),
            base_url: format!("http://{addr}"),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: crate::types::ModelCost::default(),
            context_window: 128_000,
            max_tokens: 4096,
            headers: None,
            compat: None,
        }
    }

    /// Aborting mid-stream must interrupt the SSE read immediately and mark
    /// the message `StopReason::Aborted` (matching TS openai-completions:
    /// `output.stopReason = signal.aborted ? "aborted" : "error"`).
    ///
    /// Regression guard: the old code only polled the signal per SSE chunk,
    /// so an abort while the stream was idle (no new chunk) never fired — the
    /// stream ran to completion and the message was marked `Error` instead of
    /// `Aborted`, which then triggered compaction on the next turn.
    #[tokio::test]
    async fn abort_interrupts_idle_sse_stream_and_marks_aborted() {
        // Mock server: send one thinking chunk, then hold the connection open
        // (no more data) so the SSE read blocks — exactly the "idle stream"
        // case where the old per-chunk poll never fired.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
            let body = "data: {\"id\":\"1\",\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking...\"},\"index\":0}]}\n\n";
            // No Content-Length: the connection stays open after the chunk so
            // the SSE read blocks (idle stream) until the client aborts.
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{}",
                body
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, resp.as_bytes()).await;
            // Hold the connection open — never send more data, never close.
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
        let mut stream = stream_openai(&model, &context, Some(&opts));

        // Wait until the first (thinking) chunk arrives, proving the stream is
        // mid-flight and now idle.
        let mut saw_thinking = false;
        for _ in 0..100 {
            match tokio::time::timeout(std::time::Duration::from_millis(100), stream.next()).await {
                Ok(Some(AssistantMessageEvent::ThinkingStart { .. }))
                | Ok(Some(AssistantMessageEvent::ThinkingDelta { .. })) => {
                    saw_thinking = true;
                    break;
                }
                Ok(Some(ev)) => eprintln!("unexpected event: {ev:?}"),
                Ok(None) => panic!("stream ended before abort"),
                Err(_) => {}
            }
        }
        assert!(saw_thinking, "must receive the thinking chunk first");

        // Abort while the stream is idle (blocked on the next SSE chunk).
        tx.send(true).unwrap();

        // The stream must terminate promptly with an Error(Aborted) message.
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap_or_else(|_| panic!("abort must interrupt the idle stream within 5s"))
            .unwrap_or_else(|| panic!("stream must yield an event after abort"));
        match result {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, StopReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("Request was aborted")
                );
            }
            other => panic!("expected Error(Aborted), got {other:?}"),
        }
    }
}
