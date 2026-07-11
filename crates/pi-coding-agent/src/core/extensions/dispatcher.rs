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
        AgentEvent::MessageEnd { .. } => None, // could be result-returning in phase 2
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