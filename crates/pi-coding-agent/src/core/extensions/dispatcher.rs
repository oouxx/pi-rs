//! Event payload builders + result parsing for extension dispatch.
//!
//! The aggregation logic (block short-circuit, field merge, message chain) lives
//! in JS (`__piDispatchResult` in runtime.js). This module only builds the JSON
//! payloads from `AgentEvent`/hook contexts and parses the returned JSON back
//! into the Rust hook result types.

use pi_agent_core::types::{
    AfterToolCallContext, AfterToolCallResult, BeforeToolCallContext, BeforeToolCallResult,
};

use super::runtime::ExtensionRuntime;

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

/// Parse the `__piDispatchResult("tool_call", ...)` JSON into a `BeforeToolCallResult`.
///
/// Returns `None` when no handler blocks. A runtime error is fail-open (the
/// tool call proceeds) but is logged so a broken extension can't silently
/// disable a blocking hook without a trace.
pub async fn dispatch_tool_call(
    runtime: &ExtensionRuntime,
    ctx: &BeforeToolCallContext,
) -> Option<BeforeToolCallResult> {
    let res = runtime
        .dispatch_result("tool_call", tool_call_payload(ctx))
        .await;
    let res = match res {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[pi] extension tool_call dispatch failed (fail-open): {e}");
            return None;
        }
    };
    let block = res.get("block").and_then(|v| v.as_bool()).unwrap_or(false);
    if !block {
        return None;
    }
    Some(BeforeToolCallResult {
        block: true,
        reason: res
            .get("reason")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
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

/// Parse the `__piDispatchResult("tool_result", ...)` JSON into an `AfterToolCallResult`.
///
/// Returns `None` when no handler modified anything (the runtime returns
/// `null`). A runtime error is fail-open (the original result is used
/// unchanged) but is logged so a broken extension surfaces a trace.
pub async fn dispatch_tool_result(
    runtime: &ExtensionRuntime,
    ctx: &AfterToolCallContext,
) -> Option<AfterToolCallResult> {
    let res = runtime
        .dispatch_result("tool_result", tool_result_payload(ctx))
        .await;
    let res = match res {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[pi] extension tool_result dispatch failed (fail-open): {e}");
            return None;
        }
    };
    if res.is_null() {
        return None;
    }
    let content = res
        .get("content")
        .map(|v| serde_json::from_value(v.clone()).ok())
        .flatten();
    let details = res.get("details").cloned();
    let is_error = res.get("isError").and_then(|v| v.as_bool());
    Some(AfterToolCallResult {
        content,
        details,
        is_error,
        terminate: None,
    })
}

// ============================================================================
// context — message transform before LLM call
// ============================================================================

/// Dispatch the `context` event to extensions, allowing them to modify the
/// message list before it is sent to the LLM.
///
/// The JS side (`__piDispatchResult("context", ...)`) chains handlers serially:
/// each handler sees the messages modified by the previous handler. Returns the
/// (potentially modified) messages. On error, returns the original messages
/// unchanged (fail-open).
pub async fn dispatch_context(
    runtime: &ExtensionRuntime,
    messages: Vec<pi_agent_core::types::AgentMessage>,
) -> Vec<pi_agent_core::types::AgentMessage> {
    let payload = serde_json::json!({
        "type": "context",
        "messages": messages,
    });
    let res = runtime.dispatch_result("context", payload).await;
    let res = match res {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[pi] extension context dispatch failed (fail-open): {e}");
            return messages;
        }
    };
    res
        .get("messages")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(messages)
}

// ============================================================================
// before_provider_request — modify provider request payload
// ============================================================================

/// Dispatch the `before_provider_request` event to extensions, allowing them
/// to modify the provider request payload before it is sent.
///
/// The JS side chains handlers serially: each handler sees the payload modified
/// by the previous handler. Returns the (potentially modified) payload. On
/// error, returns the original payload unchanged (fail-open).
pub async fn dispatch_before_provider_request(
    runtime: &ExtensionRuntime,
    payload: serde_json::Value,
) -> serde_json::Value {
    let event_payload = serde_json::json!({
        "type": "before_provider_request",
        "payload": payload,
    });
    let res = runtime.dispatch_result("before_provider_request", event_payload).await;
    let res = match res {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[pi] extension before_provider_request dispatch failed (fail-open): {e}");
            return payload;
        }
    };
    res
        .get("payload")
        .cloned()
        .unwrap_or(payload)
}

// ============================================================================
// user_bash — intercept/modify user `!`/`!!` commands
// ============================================================================

/// Dispatch the `user_bash` event to extensions, allowing them to intercept or
/// modify user `!`/`!!` commands.
///
/// The JS side executes handlers serially; the first handler that returns a
/// non-undefined result wins (short-circuit). Returns `None` when no handler
/// intercepts (the command proceeds normally).
pub async fn dispatch_user_bash(
    runtime: &ExtensionRuntime,
    command: &str,
    cwd: &str,
) -> Option<serde_json::Value> {
    let payload = serde_json::json!({
        "type": "user_bash",
        "command": command,
        "cwd": cwd,
    });
    let res = runtime.dispatch_result("user_bash", payload).await;
    match res {
        Ok(v) if !v.is_null() => Some(v),
        Ok(_) => None,
        Err(e) => {
            eprintln!("[pi] extension user_bash dispatch failed (fail-open): {e}");
            None
        }
    }
}

// ============================================================================
// Session lifecycle events (fire-and-forget)
// ============================================================================

/// Dispatch a `session_start` event to extensions.
pub async fn dispatch_session_start(
    runtime: &ExtensionRuntime,
    reason: &str,
) {
    let payload = serde_json::json!({
        "type": "session_start",
        "reason": reason,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_start", payload).await {
        eprintln!("[pi] session_start dispatch failed: {e}");
    }
}

/// Dispatch a `session_shutdown` event to extensions.
pub async fn dispatch_session_shutdown(
    runtime: &ExtensionRuntime,
    reason: &str,
) {
    let payload = serde_json::json!({
        "type": "session_shutdown",
        "reason": reason,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_shutdown", payload).await {
        eprintln!("[pi] session_shutdown dispatch failed: {e}");
    }
}

/// Dispatch a `session_before_switch` event to extensions.
pub async fn dispatch_session_before_switch(
    runtime: &ExtensionRuntime,
    target_session: &str,
) {
    let payload = serde_json::json!({
        "type": "session_before_switch",
        "targetSession": target_session,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_before_switch", payload).await {
        eprintln!("[pi] session_before_switch dispatch failed: {e}");
    }
}

/// Dispatch a `session_before_fork` event to extensions.
pub async fn dispatch_session_before_fork(
    runtime: &ExtensionRuntime,
    entry_id: &str,
) {
    let payload = serde_json::json!({
        "type": "session_before_fork",
        "entryId": entry_id,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_before_fork", payload).await {
        eprintln!("[pi] session_before_fork dispatch failed: {e}");
    }
}

/// Dispatch a `session_before_compact` event to extensions.
pub async fn dispatch_session_before_compact(
    runtime: &ExtensionRuntime,
    reason: &str,
) {
    let payload = serde_json::json!({
        "type": "session_before_compact",
        "reason": reason,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_before_compact", payload).await {
        eprintln!("[pi] session_before_compact dispatch failed: {e}");
    }
}

/// Dispatch a `session_compact` event to extensions.
pub async fn dispatch_session_compact(
    runtime: &ExtensionRuntime,
    summary: &str,
    tokens_before: u64,
) {
    let payload = serde_json::json!({
        "type": "session_compact",
        "summary": summary,
        "tokensBefore": tokens_before,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_compact", payload).await {
        eprintln!("[pi] session_compact dispatch failed: {e}");
    }
}

/// Dispatch a `session_before_tree` event to extensions.
pub async fn dispatch_session_before_tree(
    runtime: &ExtensionRuntime,
    reason: &str,
) {
    let payload = serde_json::json!({
        "type": "session_before_tree",
        "reason": reason,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_before_tree", payload).await {
        eprintln!("[pi] session_before_tree dispatch failed: {e}");
    }
}

/// Dispatch a `session_tree` event to extensions.
pub async fn dispatch_session_tree(
    runtime: &ExtensionRuntime,
    tree: &serde_json::Value,
) {
    let payload = serde_json::json!({
        "type": "session_tree",
        "tree": tree,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_tree", payload).await {
        eprintln!("[pi] session_tree dispatch failed: {e}");
    }
}

/// Dispatch a `session_info_changed` event to extensions.
pub async fn dispatch_session_info_changed(
    runtime: &ExtensionRuntime,
    name: Option<&str>,
) {
    let payload = serde_json::json!({
        "type": "session_info_changed",
        "name": name,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("session_info_changed", payload).await {
        eprintln!("[pi] session_info_changed dispatch failed: {e}");
    }
}

// ============================================================================
// resources_discover — extensions contribute skill/prompt/theme paths
// ============================================================================

/// Result from the `resources_discover` event.
#[derive(Debug, Default)]
pub struct ResourcesDiscoverResult {
    pub skill_paths: Vec<String>,
    pub prompt_paths: Vec<String>,
    pub theme_paths: Vec<String>,
}

/// Dispatch the `resources_discover` event to extensions, collecting
/// contributed skill/prompt/theme paths.
pub async fn dispatch_resources_discover(
    runtime: &ExtensionRuntime,
    cwd: &str,
    reason: &str,
) -> ResourcesDiscoverResult {
    let payload = serde_json::json!({
        "type": "resources_discover",
        "cwd": cwd,
        "reason": reason,
    });
    let res = runtime.dispatch_result("resources_discover", payload).await;
    let res = match res {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[pi] resources_discover dispatch failed: {e}");
            return ResourcesDiscoverResult::default();
        }
    };
    ResourcesDiscoverResult {
        skill_paths: res.get("skillPaths")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        prompt_paths: res.get("promptPaths")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        theme_paths: res.get("themePaths")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
    }
}

// ============================================================================
// project_trust — extensions participate in project trust decision
// ============================================================================

/// Result from the `project_trust` event.
#[derive(Debug)]
pub struct ProjectTrustResult {
    pub trusted: Option<bool>,
    pub remember: bool,
}

/// Dispatch the `project_trust` event to extensions, allowing them to make
/// the trust decision. Returns `None` when all handlers return "undecided"
/// (the built-in trust flow should be used).
pub async fn dispatch_project_trust(
    runtime: &ExtensionRuntime,
    cwd: &str,
) -> Option<ProjectTrustResult> {
    let payload = serde_json::json!({
        "type": "project_trust",
        "cwd": cwd,
    });
    let res = runtime.dispatch_result("project_trust", payload).await;
    let res = match res {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[pi] project_trust dispatch failed (fail-open): {e}");
            return None;
        }
    };
    if res.is_null() {
        return None;
    }
    let trusted = match res.get("trusted").and_then(|v| v.as_str()) {
        Some("yes") => Some(true),
        Some("no") => Some(false),
        _ => return None,
    };
    let remember = res.get("remember").and_then(|v| v.as_bool()).unwrap_or(false);
    Some(ProjectTrustResult { trusted, remember })
}

// ============================================================================
// after_provider_response — notification after provider HTTP response
// ============================================================================

/// Dispatch the `after_provider_response` event (fire-and-forget).
pub async fn dispatch_after_provider_response(
    runtime: &ExtensionRuntime,
    status: u16,
    headers: &std::collections::HashMap<String, String>,
) {
    let payload = serde_json::json!({
        "type": "after_provider_response",
        "status": status,
        "headers": headers,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("after_provider_response", payload).await {
        eprintln!("[pi] after_provider_response dispatch failed: {e}");
    }
}

// ============================================================================
// model_select — notification when model changes
// ============================================================================

/// Dispatch the `model_select` event (fire-and-forget).
pub async fn dispatch_model_select(
    runtime: &ExtensionRuntime,
    model: &str,
    previous_model: Option<&str>,
    source: &str,
) {
    let payload = serde_json::json!({
        "type": "model_select",
        "model": model,
        "previousModel": previous_model,
        "source": source,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("model_select", payload).await {
        eprintln!("[pi] model_select dispatch failed: {e}");
    }
}

// ============================================================================
// thinking_level_select — notification when thinking level changes
// ============================================================================

/// Dispatch the `thinking_level_select` event (fire-and-forget).
pub async fn dispatch_thinking_level_select(
    runtime: &ExtensionRuntime,
    level: &str,
    previous_level: &str,
) {
    let payload = serde_json::json!({
        "type": "thinking_level_select",
        "level": level,
        "previousLevel": previous_level,
    });
    if let Err(e) = runtime.dispatch_fire_and_forget("thinking_level_select", payload).await {
        eprintln!("[pi] thinking_level_select dispatch failed: {e}");
    }
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

/// Dispatch the `input` event to extensions, allowing them to intercept or
/// transform user input before it is processed.
///
/// The JS side chains handlers serially: each handler sees text/images modified
/// by the previous handler. `action: "handled"` short-circuits immediately.
/// `action: "transform"` modifies text and continues. `action: "continue"`
/// or undefined continues to the next handler. If all handlers return continue,
/// the final (potentially transformed) text and images are returned.
pub async fn dispatch_input(
    runtime: &ExtensionRuntime,
    text: &str,
    source: &str,
    images: Option<&[pi_agent_core::pi_ai_types::ContentBlock]>,
) -> InputEventResult {
    let payload = serde_json::json!({
        "type": "input",
        "text": text,
        "source": source,
        "images": images,
    });
    let res = runtime.dispatch_result("input", payload).await;
    let res = match res {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[pi] input dispatch failed (fail-open): {e}");
            return InputEventResult::Continue {
                text: text.to_string(),
                images: images.map(|i| i.to_vec()).unwrap_or_default(),
            };
        }
    };
    match res.get("action").and_then(|v| v.as_str()) {
        Some("handled") => InputEventResult::Handled,
        _ => {
            let text = res.get("text").and_then(|v| v.as_str()).unwrap_or(text).to_string();
            let images = res.get("images")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_else(|| images.map(|i| i.to_vec()).unwrap_or_default());
            InputEventResult::Continue { text, images }
        }
    }
}

// ============================================================================
// before_agent_start — modify context before agent loop begins
// ============================================================================

/// Dispatch the `before_agent_start` event to extensions, allowing them to
/// modify the context (system prompt, messages, tools) before the agent loop
/// begins.
///
/// Returns the (potentially modified) context. On error, returns the original
/// context unchanged (fail-open).
pub async fn dispatch_before_agent_start(
    runtime: &ExtensionRuntime,
    system_prompt: &str,
    messages: &[pi_agent_core::types::AgentMessage],
) -> Option<serde_json::Value> {
    let payload = serde_json::json!({
        "type": "before_agent_start",
        "systemPrompt": system_prompt,
        "messages": messages,
    });
    let res = runtime.dispatch_result("before_agent_start", payload).await;
    match res {
        Ok(v) if !v.is_null() => Some(v),
        Ok(_) => None,
        Err(e) => {
            eprintln!("[pi] before_agent_start dispatch failed (fail-open): {e}");
            None
        }
    }
}

// ============================================================================
// fire-and-forget / result-returning event name mapping from AgentEvent
// ============================================================================

/// Map an `AgentEvent` variant to an extension event name + payload, or `None`
/// to skip dispatch (high-frequency or result-handled variants).
///
/// Returns `(event_name, payload, is_result_returning)` where `is_result_returning`
/// indicates whether the dispatch should use `__piDispatchResult` instead of
/// `__piDispatch`.
pub fn event_from_agent_event(
    event: &pi_agent_core::types::AgentEvent,
) -> Option<(&'static str, serde_json::Value, bool)> {
    use pi_agent_core::types::AgentEvent;
    match event {
        AgentEvent::AgentStart => Some(("agent_start", serde_json::json!({}), false)),
        AgentEvent::AgentEnd { messages } => {
            Some(("agent_end", serde_json::json!({ "messages": messages }), false))
        }
        AgentEvent::TurnStart => Some(("turn_start", serde_json::json!({}), false)),
        AgentEvent::TurnEnd { message, tool_results } => Some((
            "turn_end",
            serde_json::json!({ "message": message, "toolResults": tool_results }),
            false,
        )),
        AgentEvent::MessageStart { message } => {
            Some(("message_start", serde_json::json!({ "message": message }), false))
        }
        AgentEvent::MessageUpdate { message, .. } => {
            Some(("message_update", serde_json::json!({ "message": message }), false))
        }
        AgentEvent::MessageEnd { message } => {
            // Result-returning: extensions can modify the message. The JS side
            // chains handlers serially; the modified message is returned but the
            // agent loop has already finalized it — the result is available for
            // consumers that subscribe to the dispatch result.
            Some(("message_end", serde_json::json!({ "message": message }), true))
        }
        AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => Some((
            "tool_execution_start",
            serde_json::json!({ "toolCallId": tool_call_id, "toolName": tool_name, "args": args }),
            false,
        )),
        AgentEvent::ToolExecutionUpdate { .. } => None, // high-frequency; skip
        AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error } => Some((
            "tool_execution_end",
            serde_json::json!({ "toolCallId": tool_call_id, "toolName": tool_name, "result": result, "isError": is_error }),
            false,
        )),
    }
}

/// Legacy wrapper: returns only fire-and-forget events for backward compatibility.
/// Use `event_from_agent_event` for new code that needs the result-returning flag.
pub fn fire_and_forget_from_agent_event(
    event: &pi_agent_core::types::AgentEvent,
) -> Option<(&'static str, serde_json::Value)> {
    event_from_agent_event(event).map(|(name, payload, _)| (name, payload))
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
    use pi_ai::types::Usage;

    // -----------------------------------------------------------------------
    // tool_call_payload tests
    // -----------------------------------------------------------------------

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
    fn test_tool_call_payload_empty_args() {
        let ctx = BeforeToolCallContext {
            assistant_message: AgentMessage::User {
                content: vec![],
                timestamp: 0,
            },
            tool_call: AgentToolCall {
                id: "call_456".into(),
                name: "bash".into(),
                arguments: serde_json::Value::Null,
            },
            args: serde_json::Value::Null,
            context: pi_agent_core::types::AgentContext {
                system_prompt: String::new(),
                messages: vec![],
                tools: None,
            },
        };

        let payload = tool_call_payload(&ctx);

        assert_eq!(payload["type"], "tool_call");
        assert_eq!(payload["toolCallId"], "call_456");
        assert_eq!(payload["toolName"], "bash");
        assert_eq!(payload["input"], serde_json::Value::Null);
    }

    // -----------------------------------------------------------------------
    // tool_result_payload tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_result_payload_structure() {
        let ctx = AfterToolCallContext {
            assistant_message: AgentMessage::User {
                content: vec![],
                timestamp: 0,
            },
            tool_call: AgentToolCall {
                id: "call_789".into(),
                name: "write".into(),
                arguments: serde_json::json!({"path": "/tmp/test.txt", "content": "hello"}),
            },
            args: serde_json::json!({"path": "/tmp/test.txt", "content": "hello"}),
            result: AgentToolResult {
                content: vec![
                    pi_agent_core::pi_ai_types::ContentBlock::Text {
                        text: "written".into(),
                        text_signature: None,
                    },
                ],
                details: serde_json::json!({"lines": 1}),
                terminate: None,
            },
            is_error: false,
            context: pi_agent_core::types::AgentContext {
                system_prompt: String::new(),
                messages: vec![],
                tools: None,
            },
        };

        let payload = tool_result_payload(&ctx);

        assert_eq!(payload["type"], "tool_result");
        assert_eq!(payload["toolCallId"], "call_789");
        assert_eq!(payload["toolName"], "write");
        assert_eq!(payload["isError"], false);
        assert_eq!(payload["details"]["lines"], 1);
        assert!(payload["content"].is_array());
    }

    #[test]
    fn test_tool_result_payload_error() {
        let ctx = AfterToolCallContext {
            assistant_message: AgentMessage::User {
                content: vec![],
                timestamp: 0,
            },
            tool_call: AgentToolCall {
                id: "call_err".into(),
                name: "read".into(),
                arguments: serde_json::Value::Null,
            },
            args: serde_json::Value::Null,
            result: AgentToolResult {
                content: vec![],
                details: serde_json::json!({"error": "not found"}),
                terminate: None,
            },
            is_error: true,
            context: pi_agent_core::types::AgentContext {
                system_prompt: String::new(),
                messages: vec![],
                tools: None,
            },
        };

        let payload = tool_result_payload(&ctx);

        assert_eq!(payload["isError"], true);
        assert_eq!(payload["details"]["error"], "not found");
    }

    // -----------------------------------------------------------------------
    // event_from_agent_event tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_event_from_agent_start() {
        let event = AgentEvent::AgentStart;
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, _, is_result) = result.unwrap();
        assert_eq!(name, "agent_start");
        assert!(!is_result);
    }

    #[test]
    fn test_event_from_agent_end() {
        let event = AgentEvent::AgentEnd {
            messages: vec![],
        };
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, payload, is_result) = result.unwrap();
        assert_eq!(name, "agent_end");
        assert!(!is_result);
        assert!(payload.get("messages").is_some());
    }

    #[test]
    fn test_event_from_turn_start() {
        let event = AgentEvent::TurnStart;
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, _, is_result) = result.unwrap();
        assert_eq!(name, "turn_start");
        assert!(!is_result);
    }

    #[test]
    fn test_event_from_turn_end() {
        let event = AgentEvent::TurnEnd {
            message: AgentMessage::User {
                content: vec![],
                timestamp: 0,
            },
            tool_results: vec![],
        };
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, payload, is_result) = result.unwrap();
        assert_eq!(name, "turn_end");
        assert!(!is_result);
        assert!(payload.get("message").is_some());
        assert!(payload.get("toolResults").is_some());
    }

    #[test]
    fn test_event_from_message_start() {
        let event = AgentEvent::MessageStart {
            message: AgentMessage::User {
                content: vec![],
                timestamp: 0,
            },
        };
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, payload, is_result) = result.unwrap();
        assert_eq!(name, "message_start");
        assert!(!is_result);
        assert_eq!(payload["message"]["role"], "user");
    }

    #[test]
    fn test_event_from_message_update() {
        let event = AgentEvent::MessageUpdate {
            message: AgentMessage::Assistant {
                content: vec![pi_agent_core::pi_ai_types::ContentBlock::Text {
                    text: "thinking...".into(),
                    text_signature: None,
                }],
                api: "anthropic".into(),
                provider: "anthropic".into(),
                model: "claude".into(),
                usage: Usage {
                    input: 0,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    ..Default::default()
                },
                stop_reason: None,
                error_message: None,
                timestamp: 0,
            },
            assistant_message_event: pi_agent_core::pi_ai_types::AssistantMessageEvent::Start {
                partial: pi_agent_core::pi_ai_types::AssistantMessage {
                    content: vec![],
                    model: String::new(),
                    stop_reason: pi_agent_core::pi_ai_types::StopReason::Stop,
                    usage: pi_agent_core::pi_ai_types::Usage::default(),
                    api: String::new(),
                    provider: String::new(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    error_message: None,
                    timestamp: 0,
                },
            },
        };
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, payload, is_result) = result.unwrap();
        assert_eq!(name, "message_update");
        assert!(!is_result);
        assert_eq!(payload["message"]["role"], "assistant");
    }

    #[test]
    fn test_event_from_message_end() {
        let event = AgentEvent::MessageEnd {
            message: AgentMessage::Assistant {
                content: vec![],
                api: "anthropic".into(),
                provider: "anthropic".into(),
                model: "claude".into(),
                usage: Usage {
                    input: 0,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    ..Default::default()
                },
                stop_reason: None,
                error_message: None,
                timestamp: 0,
            },
        };
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, _, is_result) = result.unwrap();
        assert_eq!(name, "message_end");
        // message_end is result-returning
        assert!(is_result);
    }

    #[test]
    fn test_event_from_tool_execution_start() {
        let event = AgentEvent::ToolExecutionStart {
            tool_call_id: "call_1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "/tmp/x"}),
        };
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, payload, is_result) = result.unwrap();
        assert_eq!(name, "tool_execution_start");
        assert!(!is_result);
        assert_eq!(payload["toolCallId"], "call_1");
        assert_eq!(payload["toolName"], "read");
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

        // High-frequency event — skipped
        assert!(result.is_none());
    }

    #[test]
    fn test_event_from_tool_execution_end() {
        let event = AgentEvent::ToolExecutionEnd {
            tool_call_id: "call_2".into(),
            tool_name: "edit".into(),
            result: serde_json::json!({"success": true}),
            is_error: false,
        };
        let result = event_from_agent_event(&event);

        assert!(result.is_some());
        let (name, payload, is_result) = result.unwrap();
        assert_eq!(name, "tool_execution_end");
        assert!(!is_result);
        assert_eq!(payload["toolCallId"], "call_2");
        assert_eq!(payload["isError"], false);
    }

    // -----------------------------------------------------------------------
    // fire_and_forget_from_agent_event tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_fire_and_forget_wrapper_drops_result_flag() {
        let event = AgentEvent::MessageEnd {
            message: AgentMessage::Assistant {
                content: vec![],
                api: "anthropic".into(),
                provider: "anthropic".into(),
                model: "claude".into(),
                usage: Usage {
                    input: 0,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    ..Default::default()
                },
                stop_reason: None,
                error_message: None,
                timestamp: 0,
            },
        };
        let result = fire_and_forget_from_agent_event(&event);

        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "message_end");
    }

    #[test]
    fn test_fire_and_forget_skips_high_frequency() {
        let event = AgentEvent::ToolExecutionUpdate {
            tool_call_id: "call_1".into(),
            tool_name: "bash".into(),
            args: serde_json::Value::Null,
            partial_result: serde_json::json!({"output": "more output"}),
        };
        let result = fire_and_forget_from_agent_event(&event);

        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // ResourcesDiscoverResult tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resources_discover_result_default() {
        let result = ResourcesDiscoverResult::default();
        assert!(result.skill_paths.is_empty());
        assert!(result.prompt_paths.is_empty());
        assert!(result.theme_paths.is_empty());
    }

    // -----------------------------------------------------------------------
    // InputEventResult tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_input_event_result_continue() {
        let result = InputEventResult::Continue {
            text: "hello".into(),
            images: vec![],
        };
        match result {
            InputEventResult::Continue { text, .. } => assert_eq!(text, "hello"),
            _ => panic!("expected Continue"),
        }
    }

    #[test]
    fn test_input_event_result_handled() {
        let result = InputEventResult::Handled;
        match result {
            InputEventResult::Handled => {} // ok
            _ => panic!("expected Handled"),
        }
    }

    // -----------------------------------------------------------------------
    // ProjectTrustResult tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_project_trust_result_yes() {
        let result = ProjectTrustResult {
            trusted: Some(true),
            remember: true,
        };
        assert_eq!(result.trusted, Some(true));
        assert!(result.remember);
    }

    #[test]
    fn test_project_trust_result_no() {
        let result = ProjectTrustResult {
            trusted: Some(false),
            remember: false,
        };
        assert_eq!(result.trusted, Some(false));
        assert!(!result.remember);
    }

    #[test]
    fn test_project_trust_result_undecided() {
        let result = ProjectTrustResult {
            trusted: None,
            remember: false,
        };
        assert!(result.trusted.is_none());
    }
}