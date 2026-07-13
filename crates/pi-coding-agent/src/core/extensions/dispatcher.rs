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
// fire-and-forget event name mapping from AgentEvent
// ============================================================================

/// Map an `AgentEvent` variant to an extension event name + payload, or `None`
/// to skip dispatch (high-frequency or result-handled variants).
pub fn fire_and_forget_from_agent_event(
    event: &pi_agent_core::types::AgentEvent,
) -> Option<(&'static str, serde_json::Value)> {
    use pi_agent_core::types::AgentEvent;
    match event {
        AgentEvent::AgentStart => Some(("agent_start", serde_json::json!({}))),
        AgentEvent::AgentEnd { messages } => {
            Some(("agent_end", serde_json::json!({ "messages": messages })))
        }
        AgentEvent::TurnStart => Some(("turn_start", serde_json::json!({}))),
        AgentEvent::TurnEnd { message, tool_results } => Some((
            "turn_end",
            serde_json::json!({ "message": message, "toolResults": tool_results }),
        )),
        AgentEvent::MessageStart { message } => {
            Some(("message_start", serde_json::json!({ "message": message })))
        }
        AgentEvent::MessageUpdate { message, .. } => {
            Some(("message_update", serde_json::json!({ "message": message })))
        }
        AgentEvent::MessageEnd { message } => {
            // Fire-and-forget dispatch for now. Full result-returning (extensions
            // replacing the message) requires a pi-agent-core hook that exposes a
            // mutable message reference before the event is finalized — tracked
            // as G2 in the alignment plan.
            Some(("message_end", serde_json::json!({ "message": message })))
        }
        AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => Some((
            "tool_execution_start",
            serde_json::json!({ "toolCallId": tool_call_id, "toolName": tool_name, "args": args }),
        )),
        AgentEvent::ToolExecutionUpdate { .. } => None, // high-frequency; skip
        AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error } => Some((
            "tool_execution_end",
            serde_json::json!({ "toolCallId": tool_call_id, "toolName": tool_name, "result": result, "isError": is_error }),
        )),
    }
}