//! Re-exports from pi_ai::types for use within pi-agent-core.
//!
//! All AI-related types live in the pi-ai crate. This module re-exports
//! them directly, plus adds a few pi-agent-core-specific types and helpers.

pub use pi_ai::types::{
    AnthropicMessagesCompat, AssistantMessage, AssistantMessageDiagnostic,
    AssistantMessageEvent, CacheRetention, ContentBlock, Context, ImagesModel,
    Message, Model, ModelCompat, ModelCost, OpenAICompletionsCompat,
    OpenAIResponsesCompat, OpenRouterRouting, ProviderResponse,
    SimpleStreamOptions, StopReason, StreamOptions, ThinkingBudgets, ThinkingLevel,
    ThinkingLevelMap, Tool, ToolCall, Transport, Usage, UsageCost, VercelGatewayRouting,
};

pub use pi_ai::models::{calculate_cost, get_model, get_models, get_providers};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ToolExecutionMode {
    Sequential,
    Parallel,
}

pub type StreamResponse = Box<dyn futures::Stream<Item = AssistantMessageEvent> + Send + Unpin>;

// ============================================================================
// Helpers
// ============================================================================

pub fn empty_usage() -> Usage {
    Usage::default()
}

pub fn create_error_tool_result(
    message: &str,
) -> crate::types::AgentToolResult<serde_json::Value> {
    crate::types::AgentToolResult {
        content: vec![text_block(message)],
        details: serde_json::Value::Object(Default::default()),
        terminate: None,
    }
}

/// Create a text ContentBlock.
pub fn text_block(text: impl Into<String>) -> ContentBlock {
    ContentBlock::Text { text: text.into(), text_signature: None }
}

/// Create a thinking ContentBlock.
pub fn thinking_block(thinking: impl Into<String>) -> ContentBlock {
    ContentBlock::Thinking { thinking: thinking.into(), thinking_signature: None, redacted: None }
}

/// Create a tool call ContentBlock.
pub fn tool_call_block(id: String, name: String, arguments: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolCall { id, name, arguments, thought_signature: None }
}

/// Create an image ContentBlock.
pub fn image_block(data: String, mime_type: String) -> ContentBlock {
    ContentBlock::Image { data, mime_type }
}

/// ModelCost helper.
pub fn model_cost(input: f64, output: f64, cache_read: f64, cache_write: f64) -> ModelCost {
    ModelCost { input, output, cache_read, cache_write }
}

/// Create an AssistantMessage without optional fields.
pub fn assistant_message(
    content: Vec<ContentBlock>,
    api: String,
    provider: String,
    model: String,
    usage: Usage,
    stop_reason: StopReason,
    timestamp: i64,
) -> AssistantMessage {
    AssistantMessage {
        content, api, provider, model,
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason,
        error_message: None,
        timestamp,
    }
}

/// Create an AssistantMessage with an error message.
pub fn assistant_message_error(
    content: Vec<ContentBlock>,
    api: String,
    provider: String,
    model: String,
    usage: Usage,
    stop_reason: StopReason,
    error_message: String,
    timestamp: i64,
) -> AssistantMessage {
    AssistantMessage {
        content, api, provider, model,
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason,
        error_message: Some(error_message),
        timestamp,
    }
}

/// Create a User Message (pi_ai type).
pub fn user_msg(content: Vec<ContentBlock>, timestamp: i64) -> Message {
    Message::User { content, timestamp }
}

/// Create an Assistant Message (pi_ai type).
pub fn assistant_msg(
    content: Vec<ContentBlock>,
    api: String,
    provider: String,
    model: String,
    usage: Usage,
    stop_reason: StopReason,
    timestamp: i64,
) -> Message {
    Message::Assistant {
        content, api, provider, model,
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason,
        error_message: None,
        timestamp,
    }
}

/// Create a ToolResult Message (pi_ai type).
pub fn tool_result_msg(
    tool_call_id: String,
    tool_name: String,
    content: Vec<ContentBlock>,
    is_error: bool,
    timestamp: i64,
) -> Message {
    Message::ToolResult { tool_call_id, tool_name, content, details: None, is_error, timestamp }
}

/// ThinkingLevel constants (pi_ai uses type alias String).
/// Matches TS `ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh"`.
pub const THINKING_OFF: &str = "off";
pub const THINKING_MINIMAL: &str = "minimal";
pub const THINKING_LOW: &str = "low";
pub const THINKING_MEDIUM: &str = "medium";
pub const THINKING_HIGH: &str = "high";
pub const THINKING_XHIGH: &str = "xhigh";

/// Create a Context with system_prompt: Option<String>.
pub fn make_context(system_prompt: String, messages: Vec<Message>, tools: Option<Vec<Tool>>) -> Context {
    Context {
        system_prompt: if system_prompt.is_empty() { None } else { Some(system_prompt) },
        messages,
        tools,
    }
}
