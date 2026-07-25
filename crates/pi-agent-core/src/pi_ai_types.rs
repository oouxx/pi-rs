#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unnecessary_struct_initialization,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_closure,
    clippy::missing_const_for_fn,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::manual_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::redundant_clone,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::match_same_arms,
    clippy::bool_to_int_with_if,
    clippy::needless_continue,
    clippy::items_after_statements,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::derive_partial_eq_without_eq,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_lit_as_bytes,
    clippy::trivially_copy_pass_by_ref,
    clippy::single_char_pattern,
    clippy::format_push_string,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::needless_raw_string_hashes,
    clippy::unnecessary_fold,
    clippy::needless_pass_by_ref_mut,
    clippy::map_identity,
    clippy::needless_return_with_question_mark,
    clippy::needless_lifetimes,
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::enum_glob_use,
    clippy::future_not_send,
    clippy::should_implement_trait,
    clippy::new_without_default,
    clippy::return_self_not_must_use,
    clippy::use_self,












    clippy::significant_drop_tightening,

    clippy::default_trait_access,

    clippy::iter_with_drain,

    clippy::if_not_else,

    clippy::explicit_iter_loop,

    clippy::assigning_clones,

    clippy::implicit_hasher,

    clippy::ignored_unit_patterns,

    clippy::missing_fields_in_debug,

    clippy::or_fun_call,

    clippy::too_long_first_doc_paragraph,

    clippy::manual_string_new,

    clippy::single_match_else,

    clippy::significant_drop_in_scrutinee,

    clippy::needless_collect,

    clippy::duplicated_attributes,

)]
//! Re-exports from pi-ai for use within pi-agent-core and downstream crates.
//!
//! All AI-related types live in the pi-ai crate. This module re-exports
//! them directly, plus adds a few pi-agent-core-specific types and helpers.

pub use pi_ai::env_api_keys::{get_env_api_key, get_env_var_name};
pub use pi_ai::types::{
    AnthropicMessagesCompat, AssistantMessage, AssistantMessageDiagnostic, AssistantMessageEvent,
    CacheRetention, ContentBlock, Context, ImagesModel, Message, Model, ModelCompat, ModelCost,
    OpenAICompletionsCompat, OpenAIResponsesCompat, OpenRouterRouting, ProviderResponse,
    SimpleStreamOptions, StopReason, StreamOptions, ThinkingBudgets, ThinkingLevel,
    ThinkingLevelMap, Tool, ToolCall, Transport, Usage, UsageCost, VercelGatewayRouting,
};

pub use pi_ai::models::{
    calculate_cost, clamp_thinking_level, get_model, get_models, get_providers,
    get_supported_thinking_levels,
};

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

pub fn create_error_tool_result(message: &str) -> crate::types::AgentToolResult<serde_json::Value> {
    crate::types::AgentToolResult {
        content: vec![text_block(message)],
        details: serde_json::Value::Object(Default::default()),
        terminate: None,
    }
}

/// Create a text ContentBlock.
pub fn text_block(text: impl Into<String>) -> ContentBlock {
    ContentBlock::Text {
        text: text.into(),
        text_signature: None,
    }
}

/// Create a thinking ContentBlock.
pub fn thinking_block(thinking: impl Into<String>) -> ContentBlock {
    ContentBlock::Thinking {
        thinking: thinking.into(),
        thinking_signature: None,
        redacted: None,
    }
}

/// Create a tool call ContentBlock.
pub fn tool_call_block(id: String, name: String, arguments: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolCall {
        id,
        name,
        arguments,
        thought_signature: None,
    }
}

/// Create an image ContentBlock.
pub fn image_block(data: String, mime_type: String) -> ContentBlock {
    ContentBlock::Image { data, mime_type }
}

/// ModelCost helper.
pub fn model_cost(input: f64, output: f64, cache_read: f64, cache_write: f64) -> ModelCost {
    ModelCost {
        input,
        output,
        cache_read,
        cache_write,
    }
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
        content,
        api,
        provider,
        model,
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
#[allow(clippy::too_many_arguments)]
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
        content,
        api,
        provider,
        model,
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
        content,
        api,
        provider,
        model,
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
    Message::ToolResult {
        tool_call_id,
        tool_name,
        content,
        details: None,
        is_error,
        timestamp,
    }
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
pub fn make_context(
    system_prompt: String,
    messages: Vec<Message>,
    tools: Option<Vec<Tool>>,
) -> Context {
    Context {
        system_prompt: if system_prompt.is_empty() {
            None
        } else {
            Some(system_prompt)
        },
        messages,
        tools,
    }
}
