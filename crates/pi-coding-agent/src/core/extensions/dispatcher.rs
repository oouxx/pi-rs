//! Event dispatch for Rust native extensions.
//!
//! Routes `AgentEvent`/hook contexts to the `ExtensionRegistry` and translates
//! results back into the Rust hook result types. Replaces the old JS-based
//! dispatch that went through deno_core + V8.

use pi_agent_core::types::{
    AfterToolCallContext, AfterToolCallResult, BeforeToolCallContext, BeforeToolCallResult,
};

use super::api::{EventResult, ExtensionContext, ExtensionEvent, ExtensionRegistry};

// ============================================================================
// tool_call (before_tool_call) — block short-circuit
// ============================================================================

/// Build the `tool_call` event payload from a `BeforeToolCallContext`.
pub fn tool_call_payload(ctx: &BeforeToolCallContext) -> serde_json::Value {
    serde_json::json!({
        "type": "tool_call",
        "toolCallId": ctx.tool_call.id,
        "toolName": ctx.tool_call.name,
        "input": ctx.args,
    })
}

/// Dispatch `tool_call` event to extensions.
///
/// Returns `Some(BeforeToolCallResult)` when an extension blocks the call.
pub async fn dispatch_tool_call(
    registry: &ExtensionRegistry,
    ctx: &BeforeToolCallContext,
    ext_ctx: &ExtensionContext,
) -> Option<BeforeToolCallResult> {
    let event = ExtensionEvent::ToolCall {
        tool_call_id: ctx.tool_call.id.clone(),
        tool_name: ctx.tool_call.name.clone(),
        input: ctx.args.clone(),
    };
    let results = registry.dispatch_event(&event, ext_ctx).await;
    for (_name, result) in &results {
        if let Some(r) = result {
            if r.block.unwrap_or(false) {
                return Some(BeforeToolCallResult {
                    block: true,
                    reason: r.reason.clone(),
                });
            }
        }
    }
    None
}

// ============================================================================
// tool_result (after_tool_call) — field merge
// ============================================================================

/// Build the `tool_result` event payload from an `AfterToolCallContext`.
pub fn tool_result_payload(ctx: &AfterToolCallContext) -> serde_json::Value {
    serde_json::json!({
        "type": "tool_result",
        "toolCallId": ctx.tool_call.id,
        "toolName": ctx.tool_call.name,
        "input": ctx.args,
        "content": ctx.result.content,
        "details": ctx.result.details,
        "isError": ctx.is_error,
    })
}

/// Dispatch `tool_result` event to extensions.
///
/// Merges content, details, and isError modifications from all extension
/// handlers, matching the TS emitToolResult() behavior.
/// Returns `Some(AfterToolCallResult)` when an extension modifies the result.
pub async fn dispatch_tool_result(
    registry: &ExtensionRegistry,
    ctx: &AfterToolCallContext,
    ext_ctx: &ExtensionContext,
) -> Option<AfterToolCallResult> {
    let event = ExtensionEvent::ToolResult {
        tool_call_id: ctx.tool_call.id.clone(),
        tool_name: ctx.tool_call.name.clone(),
        input: ctx.args.clone(),
        content: serde_json::to_value(&ctx.result.content).map(|v| match v {
            serde_json::Value::Array(arr) => arr,
            _ => vec![],
        }).unwrap_or_default(),
        is_error: ctx.is_error,
    };
    let results = registry.dispatch_event(&event, ext_ctx).await;
    let mut merged_content: Option<Vec<serde_json::Value>> = None;
    let mut merged_details: Option<serde_json::Value> = None;
    let mut merged_is_error: Option<bool> = None;
    let mut blocked = false;

    for (_name, result) in &results {
        if let Some(r) = result {
            if r.block.unwrap_or(false) {
                blocked = true;
            }
            if let Some(content) = &r.content {
                merged_content = Some(content.clone());
            }
            if let Some(details) = &r.details {
                merged_details = Some(details.clone());
            }
            if let Some(is_error) = r.is_error {
                merged_is_error = Some(is_error);
            }
        }
    }

    if blocked || merged_content.is_some() || merged_details.is_some() || merged_is_error.is_some() {
        Some(AfterToolCallResult {
            content: merged_content.map(|v| {
                v.into_iter().filter_map(|val| {
                    serde_json::from_value(val).ok()
                }).collect()
            }),
            details: merged_details,
            is_error: merged_is_error,
            terminate: None,
        })
    } else {
        None
    }
}

// ============================================================================
// context — message transform before LLM call
// ============================================================================

/// Dispatch the `context` event to extensions, allowing them to modify the
/// message list before it is sent to the LLM.
///
/// Chains modifications through all extensions: each handler sees the result
/// of the previous handler, matching the TS emitContext() behavior.
pub async fn dispatch_context(
    registry: &ExtensionRegistry,
    messages: Vec<pi_agent_core::types::AgentMessage>,
    ext_ctx: &ExtensionContext,
) -> Vec<pi_agent_core::types::AgentMessage> {
    let mut current_messages = messages;
    let event = ExtensionEvent::Context {
        messages: serde_json::to_value(&current_messages).ok()
            .and_then(|v| match v { serde_json::Value::Array(arr) => Some(arr), _ => None })
            .unwrap_or_default(),
    };
    let results = registry.dispatch_event(&event, ext_ctx).await;
    for (_name, result) in &results {
        if let Some(r) = result {
            if let Some(msgs) = &r.messages {
                if let Ok(parsed) = serde_json::from_value(serde_json::Value::Array(msgs.clone())) {
                    current_messages = parsed;
                }
            }
        }
    }
    current_messages
}

// ============================================================================
// before_provider_request — modify provider request payload
// ============================================================================

/// Dispatch the `before_provider_request` event to extensions.
///
/// Extensions can modify the provider request payload. Returns the (potentially
/// modified) payload. The last extension's modification wins.
pub async fn dispatch_before_provider_request(
    registry: &ExtensionRegistry,
    payload: serde_json::Value,
    ext_ctx: &ExtensionContext,
) -> serde_json::Value {
    let event = ExtensionEvent::BeforeProviderRequest {
        payload: payload.clone(),
    };
    let results = registry.dispatch_event(&event, ext_ctx).await;
    let mut result_payload = payload;
    for (_name, result) in &results {
        if let Some(r) = result {
            if let Some(p) = &r.payload {
                result_payload = p.clone();
            }
        }
    }
    result_payload
}

// ============================================================================
// Session lifecycle events (fire-and-forget)
// ============================================================================

/// Dispatch a `session_start` event to extensions.
pub async fn dispatch_session_start(
    registry: &ExtensionRegistry,
    reason: &str,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::SessionStart {
        reason: reason.to_string(),
        previous_session_file: None,
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

/// Dispatch a `session_shutdown` event to extensions.
pub async fn dispatch_session_shutdown(
    registry: &ExtensionRegistry,
    reason: &str,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::SessionShutdown {
        reason: reason.to_string(),
        target_session_file: None,
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

/// Dispatch a `session_before_compact` event to extensions.
pub async fn dispatch_session_before_compact(
    registry: &ExtensionRegistry,
    reason: &str,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::SessionBeforeCompact {
        reason: reason.to_string(),
        will_retry: false,
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

/// Dispatch a `session_compact` event to extensions.
pub async fn dispatch_session_compact(
    registry: &ExtensionRegistry,
    summary: &str,
    tokens_before: u64,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::SessionCompact {
        summary: summary.to_string(),
        tokens_before,
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

/// Dispatch a `session_before_switch` event to extensions.
pub async fn dispatch_session_before_switch(
    registry: &ExtensionRegistry,
    target_session: &str,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::SessionBeforeSwitch {
        reason: "resume".to_string(),
        target_session_file: Some(target_session.to_string()),
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

/// Dispatch a `session_before_fork` event to extensions.
pub async fn dispatch_session_before_fork(
    registry: &ExtensionRegistry,
    entry_id: &str,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::SessionBeforeFork {
        entry_id: entry_id.to_string(),
        position: "before".to_string(),
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

/// Dispatch a `session_before_tree` event to extensions.
pub async fn dispatch_session_before_tree(
    registry: &ExtensionRegistry,
    target_id: &str,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::SessionBeforeTree {
        target_id: target_id.to_string(),
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

/// Dispatch a `session_info_changed` event to extensions.
pub async fn dispatch_session_info_changed(
    registry: &ExtensionRegistry,
    name: Option<&str>,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::SessionInfoChanged {
        name: name.map(String::from),
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

// ============================================================================
// before_agent_start — modify context before agent loop begins
// ============================================================================

/// Dispatch the `before_agent_start` event to extensions.
///
/// Chains system_prompt modifications through all extensions and collects
/// messages from all handlers, matching the TS emitBeforeAgentStart() behavior.
/// Returns the last extension's system_prompt override, if any.
pub async fn dispatch_before_agent_start(
    registry: &ExtensionRegistry,
    system_prompt: &str,
    messages: &[pi_agent_core::types::AgentMessage],
    ext_ctx: &ExtensionContext,
) -> Option<serde_json::Value> {
    let event = ExtensionEvent::BeforeAgentStart {
        prompt: messages.iter()
            .filter_map(|m| {
                if let pi_agent_core::types::AgentMessage::User { content, .. } = m {
                    Some(content.iter()
                        .filter_map(|c| {
                            if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = c {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<&str>>()
                        .join("\n"))
                } else {
                    None
                }
            })
            .collect::<Vec<String>>()
            .join("\n"),
        system_prompt: system_prompt.to_string(),
    };
    let results = registry.dispatch_event(&event, ext_ctx).await;
    let mut last_system_prompt: Option<String> = None;
    for (_name, result) in &results {
        if let Some(r) = result {
            if let Some(sp) = &r.system_prompt {
                last_system_prompt = Some(sp.clone());
            }
        }
    }
    last_system_prompt.map(|sp| serde_json::json!({ "systemPrompt": sp }))
}

// ============================================================================
// input — intercept/transform user input before processing
// ============================================================================

/// Result from the `input` event.
#[derive(Debug)]
pub enum InputEventResult {
    /// Continue with the (potentially transformed) text and images.
    Continue {
        text: String,
        images: Vec<pi_agent_core::pi_ai_types::ContentBlock>,
    },
    /// Extension handled the input; discard it.
    Handled,
}

/// Dispatch the `input` event to extensions.
///
/// Chains transforms through all extensions: each handler sees the transformed
/// text from the previous handler, matching the TS emitInput() behavior.
/// Returns Handled if any extension handles the input.
pub async fn dispatch_input(
    registry: &ExtensionRegistry,
    text: &str,
    source: &str,
    images: Option<&[pi_agent_core::pi_ai_types::ContentBlock]>,
    ext_ctx: &ExtensionContext,
) -> InputEventResult {
    let mut current_text = text.to_string();
    let mut current_images = images.map(|i| i.to_vec()).unwrap_or_default();

    let event = ExtensionEvent::Input {
        text: current_text.clone(),
        source: source.to_string(),
    };
    let results = registry.dispatch_event(&event, ext_ctx).await;
    for (_name, result) in &results {
        if let Some(r) = result {
            match r.action.as_deref() {
                Some("handled") => return InputEventResult::Handled,
                Some("transform") => {
                    if let Some(t) = &r.text {
                        current_text = t.clone();
                    }
                    if let Some(imgs) = &r.images {
                        current_images = imgs.iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect();
                    }
                }
                _ => {}
            }
        }
    }
    InputEventResult::Continue {
        text: current_text,
        images: current_images,
    }
}

// ============================================================================
// model_select / thinking_level_select
// ============================================================================

/// Dispatch the `model_select` event.
pub async fn dispatch_model_select(
    registry: &ExtensionRegistry,
    model: &str,
    previous_model: Option<&str>,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::ModelSelect {
        model: model.to_string(),
        previous_model: previous_model.map(String::from),
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

/// Dispatch the `thinking_level_select` event.
pub async fn dispatch_thinking_level_select(
    registry: &ExtensionRegistry,
    level: &str,
    previous_level: &str,
    ext_ctx: &ExtensionContext,
) {
    let event = ExtensionEvent::ThinkingLevelSelect {
        level: level.to_string(),
        previous_level: previous_level.to_string(),
    };
    registry.dispatch_event(&event, ext_ctx).await;
}

// ============================================================================
// Event name mapping from AgentEvent
// ============================================================================

/// Map an `AgentEvent` variant to an extension event, or `None` to skip.
pub fn event_from_agent_event(
    event: &pi_agent_core::types::AgentEvent,
) -> Option<ExtensionEvent> {
    use pi_agent_core::types::AgentEvent;
    match event {
        AgentEvent::AgentStart => Some(ExtensionEvent::AgentStart),
        AgentEvent::AgentEnd { messages } => Some(ExtensionEvent::AgentEnd {
            messages: serde_json::to_value(messages).ok()
                .and_then(|v| match v { serde_json::Value::Array(arr) => Some(arr), _ => None })
                .unwrap_or_default(),
        }),
        AgentEvent::TurnStart => Some(ExtensionEvent::TurnStart),
        AgentEvent::TurnEnd { message, tool_results } => Some(ExtensionEvent::TurnEnd {
            message: serde_json::to_value(message).unwrap_or_default(),
            tool_results: serde_json::to_value(tool_results).ok()
                .and_then(|v| match v { serde_json::Value::Array(arr) => Some(arr), _ => None })
                .unwrap_or_default(),
        }),
        AgentEvent::MessageStart { message } => Some(ExtensionEvent::MessageStart {
            message: serde_json::to_value(message).unwrap_or_default(),
        }),
        AgentEvent::MessageUpdate { message, .. } => Some(ExtensionEvent::MessageUpdate {
            message: serde_json::to_value(message).unwrap_or_default(),
        }),
        AgentEvent::MessageEnd { message } => Some(ExtensionEvent::MessageEnd {
            message: serde_json::to_value(message).unwrap_or_default(),
        }),
        AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
            Some(ExtensionEvent::ToolExecutionStart {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
            })
        }
        AgentEvent::ToolExecutionUpdate { .. } => None, // high-frequency; skip
        AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error } => {
            Some(ExtensionEvent::ToolExecutionEnd {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                result: result.clone(),
                is_error: *is_error,
            })
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pi_agent_core::types::{
        AgentEvent, AgentMessage, AgentToolCall, AgentToolResult, BeforeToolCallContext,
    };

    #[test]
    fn test_tool_call_payload_structure() {
        let ctx = BeforeToolCallContext {
            assistant_message: AgentMessage::User {
                content: vec![],
                timestamp: 0,
            },
            tool_call: AgentToolCall {
                id: "call_123".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "/tmp/test.txt"}),
            },
            args: serde_json::json!({"path": "/tmp/test.txt"}),
            context: pi_agent_core::types::AgentContext {
                system_prompt: String::new(),
                messages: vec![],
                tools: None,
            },
        };

        let payload = tool_call_payload(&ctx);

        assert_eq!(payload["type"], "tool_call");
        assert_eq!(payload["toolCallId"], "call_123");
        assert_eq!(payload["toolName"], "read");
        assert_eq!(payload["input"]["path"], "/tmp/test.txt");
    }

    #[test]
    fn test_event_from_agent_start() {
        let event = AgentEvent::AgentStart;
        let result = event_from_agent_event(&event);
        assert!(result.is_some());
    }

    #[test]
    fn test_event_from_agent_end() {
        let event = AgentEvent::AgentEnd {
            messages: vec![],
        };
        let result = event_from_agent_event(&event);
        assert!(result.is_some());
    }

    #[test]
    fn test_event_from_turn_start() {
        let event = AgentEvent::TurnStart;
        let result = event_from_agent_event(&event);
        assert!(result.is_some());
    }

    #[test]
    fn test_event_from_tool_execution_update_skipped() {
        let event = AgentEvent::ToolExecutionUpdate {
            tool_call_id: "call_1".into(),
            tool_name: "bash".into(),
            args: serde_json::Value::Null,
            partial_result: serde_json::json!({"output": "output"}),
        };
        let result = event_from_agent_event(&event);
        assert!(result.is_none());
    }
}
