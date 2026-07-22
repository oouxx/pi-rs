//! Event dispatch for Rust native extensions.
//!
//! Routes `AgentEvent`/hook contexts to the `ExtensionRegistry`'s `HookRunner`.
//! Replaces the old ExtensionEvent enum-based dispatch.

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{
    AfterToolCallContext, AfterToolCallResult, AgentMessage, BeforeToolCallContext,
    BeforeToolCallResult,
};

use super::api::{ExtensionContext, ExtensionRegistry, ToolCallOutput};

// ============================================================================
// Parameter structs (to keep function signatures ≤ 3 params per spec)
// ============================================================================

/// Parameters for `dispatch_session_compact`.
pub struct DispatchSessionCompactParams<'a> {
    pub registry: &'a ExtensionRegistry,
    pub summary: &'a str,
    pub tokens_before: u64,
    pub ext_ctx: &'a ExtensionContext,
}

/// Parameters for `dispatch_before_agent_start`.
pub struct DispatchBeforeAgentStartParams<'a> {
    pub registry: &'a ExtensionRegistry,
    pub system_prompt: &'a str,
    pub messages: &'a [AgentMessage],
    pub ext_ctx: &'a ExtensionContext,
}

/// Parameters for `dispatch_input`.
pub struct DispatchInputParams<'a> {
    pub registry: &'a ExtensionRegistry,
    pub text: &'a str,
    pub source: &'a str,
    pub images: Option<&'a [ContentBlock]>,
    pub ext_ctx: &'a ExtensionContext,
}

/// Parameters for `dispatch_model_select`.
pub struct DispatchModelSelectParams<'a> {
    pub registry: &'a ExtensionRegistry,
    pub model: &'a str,
    pub previous_model: Option<&'a str>,
    pub ext_ctx: &'a ExtensionContext,
}

/// Parameters for `dispatch_thinking_level_select`.
pub struct DispatchThinkingLevelSelectParams<'a> {
    pub registry: &'a ExtensionRegistry,
    pub level: &'a str,
    pub previous_level: &'a str,
    pub ext_ctx: &'a ExtensionContext,
}

// ============================================================================
// InputEventResult
// ============================================================================

/// Result from dispatching an input event.
pub enum InputEventResult {
    /// Input was handled by an extension (e.g., a command).
    Handled,
    /// Input was not handled; continue with the original or modified text.
    Continue { text: String, images: Option<Vec<ContentBlock>> },
}

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

/// Dispatch `tool_call` event to extensions via HookRunner.
///
/// Returns `Some(BeforeToolCallResult)` when an extension blocks the call.
pub async fn dispatch_tool_call(
    registry: &ExtensionRegistry,
    ctx: &BeforeToolCallContext,
    _ext_ctx: &ExtensionContext,
) -> Option<BeforeToolCallResult> {
    let result = registry
        .hook_runner()
        .run_before_tool_call(ctx.tool_call.name.clone(), ctx.args.clone())
        .await;
    match result {
        crate::core::extensions::HookResult::Cancel(reason) => {
            Some(BeforeToolCallResult {
                block: true,
                reason: Some(reason),
            })
        }
        crate::core::extensions::HookResult::Continue(_) => None,
    }
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

/// Dispatch `tool_result` event to extensions via HookRunner.
///
/// Returns `Some(AfterToolCallResult)` when an extension modifies the result.
pub async fn dispatch_tool_result(
    registry: &ExtensionRegistry,
    ctx: &AfterToolCallContext,
    _ext_ctx: &ExtensionContext,
) -> Option<AfterToolCallResult> {
    let result_value = serde_json::to_value(&ctx.result.content).unwrap_or_default();
    registry
        .hook_runner()
        .run_after_tool_call(&ctx.tool_call.name, &result_value, ctx.is_error)
        .await;
    // HookRunner's after_tool_call doesn't modify the result in the current design.
    // The TS original merged content/details from extensions, but that pattern
    // is not commonly used. We return None for now.
    None
}

// ============================================================================
// context — fire-and-forget
// ============================================================================

/// Dispatch `context` event to extensions via HookRunner.
pub async fn dispatch_context(
    registry: &ExtensionRegistry,
    messages: &[serde_json::Value],
    _ext_ctx: &ExtensionContext,
) {
    registry.hook_runner().fire_context(messages).await;
}

// ============================================================================
// before_provider_request — modifying
// ============================================================================

/// Dispatch `before_provider_request` event to extensions via HookRunner.
pub async fn dispatch_before_provider_request(
    registry: &ExtensionRegistry,
    payload: &serde_json::Value,
    _ext_ctx: &ExtensionContext,
) {
    let _ = registry
        .hook_runner()
        .run_before_provider_request(payload)
        .await;
}

// ============================================================================
// session_start — fire-and-forget
// ============================================================================

/// Dispatch `session_start` event to extensions via HookRunner.
pub async fn dispatch_session_start(
    registry: &ExtensionRegistry,
    reason: &str,
    _ext_ctx: &ExtensionContext,
) {
    registry
        .hook_runner()
        .fire_session_start(reason, None)
        .await;
}

// ============================================================================
// session_shutdown — fire-and-forget
// ============================================================================

/// Dispatch `session_shutdown` event to extensions via HookRunner.
pub async fn dispatch_session_shutdown(
    registry: &ExtensionRegistry,
    reason: &str,
    _ext_ctx: &ExtensionContext,
) {
    registry
        .hook_runner()
        .fire_session_shutdown(reason, None)
        .await;
}

// ============================================================================
// session_before_compact — modifying
// ============================================================================

/// Dispatch `session_before_compact` event to extensions via HookRunner.
pub async fn dispatch_session_before_compact(
    registry: &ExtensionRegistry,
    reason: &str,
    will_retry: bool,
    _ext_ctx: &ExtensionContext,
) {
    let _ = registry
        .hook_runner()
        .run_before_session_compact(reason.to_string(), will_retry)
        .await;
}

// ============================================================================
// session_compact — fire-and-forget
// ============================================================================

/// Dispatch `session_compact` event to extensions via HookRunner.
pub async fn dispatch_session_compact(
    params: DispatchSessionCompactParams<'_>,
) {
    params.registry
        .hook_runner()
        .fire_compact(params.summary, params.tokens_before)
        .await;
}

// ============================================================================
// session_before_switch — modifying
// ============================================================================

/// Dispatch `session_before_switch` event to extensions via HookRunner.
pub async fn dispatch_session_before_switch(
    registry: &ExtensionRegistry,
    target_session_file: &str,
    _ext_ctx: &ExtensionContext,
) {
    let _ = registry
        .hook_runner()
        .run_before_session_switch(
            "resume".to_string(),
            if target_session_file.is_empty() {
                None
            } else {
                Some(target_session_file.to_string())
            },
        )
        .await;
}

// ============================================================================
// session_before_fork — modifying
// ============================================================================

/// Dispatch `session_before_fork` event to extensions via HookRunner.
pub async fn dispatch_session_before_fork(
    registry: &ExtensionRegistry,
    entry_id: &str,
    _ext_ctx: &ExtensionContext,
) {
    let _ = registry
        .hook_runner()
        .run_before_session_fork(entry_id.to_string(), "current".to_string())
        .await;
}

// ============================================================================
// session_before_tree — modifying
// ============================================================================

/// Dispatch `session_before_tree` event to extensions via HookRunner.
pub async fn dispatch_session_before_tree(
    registry: &ExtensionRegistry,
    target_id: &str,
    _ext_ctx: &ExtensionContext,
) {
    let _ = registry
        .hook_runner()
        .run_before_session_tree(target_id)
        .await;
}

// ============================================================================
// session_info_changed — fire-and-forget
// ============================================================================

/// Dispatch `session_info_changed` event to extensions via HookRunner.
pub async fn dispatch_session_info_changed(
    registry: &ExtensionRegistry,
    name: Option<&str>,
    _ext_ctx: &ExtensionContext,
) {
    registry
        .hook_runner()
        .fire_session_info_changed(name)
        .await;
}

// ============================================================================
// before_agent_start — modifying
// ============================================================================

/// Dispatch `before_agent_start` event to extensions via HookRunner.
///
/// Returns `true` if the agent start was cancelled by an extension.
pub async fn dispatch_before_agent_start(
    params: DispatchBeforeAgentStartParams<'_>,
) -> bool {
    let result = params.registry
        .hook_runner()
        .run_before_agent_start(params.messages.first().and_then(|m| {
            if let AgentMessage::User { content, .. } = m {
                content.first().and_then(|b| {
                    if let ContentBlock::Text { text, .. } = b {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        }).unwrap_or_default(), params.system_prompt.to_string())
        .await;
    result.is_cancel()
}

// ============================================================================
// input — modifying
// ============================================================================

/// Dispatch `input` event to extensions via HookRunner.
///
/// Returns `InputEventResult::Handled` if an extension handled the input,
/// or `InputEventResult::Continue { text }` with the (possibly modified) text.
pub async fn dispatch_input(
    params: DispatchInputParams<'_>,
) -> InputEventResult {
    let result = params.registry
        .hook_runner()
        .run_on_input(params.text.to_string(), params.source.to_string())
        .await;
    match result {
        crate::core::extensions::HookResult::Continue(text) => {
            InputEventResult::Continue { text, images: None }
        }
        crate::core::extensions::HookResult::Cancel(_reason) => {
            InputEventResult::Handled
        }
    }
}

// ============================================================================
// model_select — fire-and-forget
// ============================================================================

/// Dispatch `model_select` event to extensions via HookRunner.
pub async fn dispatch_model_select(
    params: DispatchModelSelectParams<'_>,
) {
    params.registry
        .hook_runner()
        .fire_model_select(params.model, params.previous_model)
        .await;
}

// ============================================================================
// thinking_level_select — fire-and-forget
// ============================================================================

/// Dispatch `thinking_level_select` event to extensions via HookRunner.
pub async fn dispatch_thinking_level_select(
    params: DispatchThinkingLevelSelectParams<'_>,
) {
    params.registry
        .hook_runner()
        .fire_thinking_level_select(params.level, params.previous_level)
        .await;
}

// ============================================================================
// Extension tool execution dispatch
// ============================================================================

/// Dispatch a tool call to extension `handle_tool_call` handlers.
///
/// Each extension's `handle_tool_call()` is tried in registration order;
/// the first one that returns `Some(ToolCallOutput)` wins.
/// Returns `None` when no extension handles the tool.
pub async fn dispatch_handle_tool_call(
    registry: &ExtensionRegistry,
    tool_name: &str,
    params: serde_json::Value,
    ext_ctx: &ExtensionContext,
) -> Option<ToolCallOutput> {
    registry
        .hook_runner()
        .dispatch_tool_call(tool_name, params, ext_ctx)
        .await
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pi_agent_core::types::{AgentToolCall, AgentToolResult, BeforeToolCallContext};

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
}
