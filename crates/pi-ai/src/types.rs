use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ============================================================================
// Content block types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redacted: Option<bool>,
    },
    #[serde(rename = "toolCall")]
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            text_signature: None,
        }
    }
}

// ============================================================================
// ToolCall (standalone, used in events)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    #[serde(rename = "type")]
    pub type_field: String,
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

impl ToolCall {
    #[must_use]
    pub fn new(id: String, name: String, arguments: serde_json::Value) -> Self {
        Self {
            type_field: "toolCall".to_string(),
            id,
            name,
            arguments,
            thought_signature: None,
        }
    }
}

// ============================================================================
// Usage and cost types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UsageCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(default)]
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
    #[serde(default)]
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(default)]
    #[serde(rename = "cacheWrite")]
    pub cache_write: u64,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "cacheWrite1h")]
    pub cache_write_1h: Option<u64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    #[serde(default)]
    #[serde(rename = "totalTokens")]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: UsageCost,
}

// ============================================================================
// Stop reason
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    Stop,
    Length,
    #[serde(rename = "toolUse")]
    ToolUse,
    Error,
    Aborted,
}

// ============================================================================
// Messages
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User {
        content: Vec<ContentBlock>,
        timestamp: i64,
    },
    #[serde(rename = "assistant")]
    Assistant {
        content: Vec<ContentBlock>,
        api: String,
        provider: String,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "responseModel")]
        response_model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "responseId")]
        response_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
        usage: Usage,
        #[serde(rename = "stopReason")]
        stop_reason: StopReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "errorMessage")]
        error_message: Option<String>,
        timestamp: i64,
    },
    #[serde(rename = "toolResult")]
    ToolResult {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        content: Vec<ContentBlock>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(rename = "isError")]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "addedToolNames")]
        added_tool_names: Option<Vec<String>>,
        timestamp: i64,
    },
}

// ============================================================================
// Assistant message (used in events)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    pub api: String,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "responseModel")]
    pub response_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "responseId")]
    pub response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    pub usage: Usage,
    #[serde(rename = "stopReason")]
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "errorMessage")]
    pub error_message: Option<String>,
    /// Raw provider stop reason (e.g. `completed`, `incomplete.max_output_tokens`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rawStopReason")]
    pub raw_stop_reason: Option<String>,
    pub timestamp: i64,
}

// ============================================================================
// Stream events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantMessageEvent {
    #[serde(rename = "start")]
    Start { partial: AssistantMessage },
    #[serde(rename = "text_start")]
    TextStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        partial: AssistantMessage,
    },
    #[serde(rename = "text_delta")]
    TextDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "text_end")]
    TextEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_start")]
    ThinkingStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_end")]
    ThinkingEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_start")]
    ToolCallStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_end")]
    ToolCallEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        #[serde(rename = "toolCall")]
        tool_call: ToolCall,
        partial: AssistantMessage,
    },
    #[serde(rename = "done")]
    Done {
        reason: StopReason,
        message: AssistantMessage,
    },
    #[serde(rename = "error")]
    Error {
        reason: StopReason,
        error: AssistantMessage,
    },
}

// ============================================================================
// Diagnostics
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticErrorInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<serde_json::Value>,
}

/// Structured diagnostic attached to an assistant message
/// (match TS `utils/diagnostics.ts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessageDiagnostic {
    #[serde(rename = "type")]
    pub type_field: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DiagnosticErrorInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Map<String, serde_json::Value>>,
}

// ============================================================================
// Model definition
// ============================================================================

/// Known API protocol identifiers. Use as `&'static str` for known values
/// or `String` for custom APIs via the `Api` type alias.
pub type Api = String;

/// Known provider identifiers. Use as `&'static str` for known values
/// or `String` for custom providers via the `Provider` type alias.
pub type Provider = String;

pub type ThinkingLevel = String;
/// Valid values: "off", "minimal", "low", "medium", "high", "xhigh"
pub type ThinkingLevelMap = std::collections::HashMap<String, Option<String>>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThinkingBudgets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimal: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Sse,
    Websocket,
    #[serde(rename = "websocket-cached")]
    WebsocketCached,
    Auto,
}

// ============================================================================
// OpenAI completions compatibility
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompletionsCompat {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_store: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_usage_in_streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_tool_result_name: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_assistant_after_tool_result: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_thinking_as_text: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_reasoning_content_on_assistant_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_router_routing: Option<OpenRouterRouting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vercel_gateway_routing: Option<VercelGatewayRouting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zai_tool_stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_session_affinity_headers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_finish_reason: Option<bool>,
    /// vLLM/HF chat-template thinking kwargs (match TS `chatTemplateKwargs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<serde_json::Map<String, serde_json::Value>>,
    /// Baseten chat-template args (match TS `chatTemplateArgs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_args: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_thinking_token_budget: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_openai_grammar_tools: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred_tools_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity_format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIResponsesCompat {
    /// Whether the provider supports the `developer` role (vs `system`). Default: true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    /// Session-affinity header format. Default: auto-detected from provider/baseUrl.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity_format: Option<SessionAffinityFormat>,
    /// Whether the provider supports `prompt_cache_retention: "24h"`. Default: true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    /// Whether the provider supports strict JSON-schema function tools. Default: false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    /// Whether to emit OpenAI custom tools with grammar formats. Default: false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_openai_grammar_tools: Option<bool>,
    /// Whether the model supports client-executed tool search for deferred tools. Default: false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_tool_search: Option<bool>,
    /// Whether the provider supports explicit prompt-cache mode. Default: false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_explicit_prompt_cache_mode: Option<bool>,
}

/// Session-affinity header format for OpenAI Responses providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionAffinityFormat {
    #[serde(rename = "openai")]
    Openai,
    #[serde(rename = "openai-nosession")]
    OpenaiNosession,
    #[serde(rename = "openrouter")]
    Openrouter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicMessagesCompat {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_eager_tool_input_streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_session_affinity_headers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_cache_control_on_tools: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_adaptive_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_empty_signature: Option<bool>,
}

// ============================================================================
// Routing types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterRouting {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_collection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zdr: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enforce_distillable_text: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantizations: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_price: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_min_throughput: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_max_latency: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VercelGatewayRouting {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
}

// ============================================================================
// Model with compat typing
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(default)]
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
    #[serde(default)]
    pub tiers: Vec<ModelCostTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostTier {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(default)]
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
    #[serde(rename = "inputTokensAbove")]
    pub input_tokens_above: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub reasoning: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "thinkingLevelMap")]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    pub input: Vec<String>,
    pub cost: ModelCost,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
    #[serde(rename = "maxTokens")]
    pub max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compat: Option<ModelCompat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ModelCompat {
    OpenAICompletions(Box<OpenAICompletionsCompat>),
    OpenAIResponses(OpenAIResponsesCompat),
    AnthropicMessages(AnthropicMessagesCompat),
}

// ============================================================================
// Tool types
// ============================================================================

/// Grammar format variants for constrained sampling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrammarVariants {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_lark: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_regex: Option<String>,
}

/// Optional provider-side constrained sampling config for a tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstrainedSamplingConfig {
    JsonSchema { strict: JsonSchemaStrict },
    Grammar { variants: GrammarVariants },
}

/// Strictness preference for JSON-schema constrained sampling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JsonSchemaStrict {
    Prefer,
    Require,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<ConstrainedSamplingConfig>,
}

// ============================================================================
// Context
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "systemPrompt")]
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

// ============================================================================
// Stream options
// ============================================================================

// ============================================================================
// Tool choice
// ============================================================================

/// Controls tool selection behavior for the model.
/// Matches the `OpenAI` `tool_choice` parameter format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// Auto, None, or Required
    Mode(ToolChoiceMode),
    /// A specific function call
    Specific {
        #[serde(rename = "type")]
        type_field: String,
        function: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoiceMode {
    Auto,
    None,
    Required,
}

// ============================================================================
// Stream options
// ============================================================================

#[allow(clippy::type_complexity)]
pub struct StreamOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub api_key: Option<String>,
    pub transport: Option<Transport>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub websocket_connect_timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub metadata: Option<serde_json::Value>,
    /// Callback invoked before the provider request is sent.
    /// Receives the request payload and returns the (possibly modified) payload.
    /// Return `None` to cancel the request.
    pub on_payload: Option<
        Arc<
            dyn Fn(
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send>,
                > + Send
                + Sync,
        >,
    >,
    /// Callback invoked before provider HTTP request headers are sent.
    /// Receives the current headers map and returns the (possibly modified) headers.
    pub on_headers: Option<
        Arc<
            dyn Fn(
                    std::collections::HashMap<String, String>,
                ) -> std::pin::Pin<
                    Box<
                        dyn std::future::Future<Output = std::collections::HashMap<String, String>>
                            + Send,
                    >,
                > + Send
                + Sync,
        >,
    >,
    /// Callback invoked after a provider HTTP response is received.
    /// Receives the HTTP status code and response headers.
    pub on_provider_response:
        Option<Arc<dyn Fn(u16, std::collections::HashMap<String, String>) + Send + Sync>>,

    pub tool_choice: Option<ToolChoice>,
    /// Service tier for OpenAI Responses (`flex` / `priority`).
    pub service_tier: Option<String>,
    /// Reasoning effort level (`minimal`/`low`/`medium`/`high`/`off`/...) passed
    /// through to providers that support `reasoning_effort`.
    pub reasoning_effort: Option<String>,
    /// Per-level thinking token budgets for vLLM `thinking_token_budget`.
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// Ask the backend for debug metadata (e.g. routing response headers);
    /// pi-messages appends `?debug=1` (match TS `PiMessagesOptions.debug`).
    pub debug: Option<bool>,
}

impl std::fmt::Debug for StreamOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamOptions")
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("api_key", &self.api_key.as_ref().map(|_| "..."))
            .field("session_id", &self.session_id)
            .field("headers", &self.headers)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_retries", &self.max_retries)
            .field("max_retry_delay_ms", &self.max_retry_delay_ms)
            .field("metadata", &self.metadata)
            .field("tool_choice", &self.tool_choice)
            .field("signal", &self.signal.as_ref().map(|_| "..."))
            .field("transport", &self.transport)
            .field("cache_retention", &self.cache_retention)
            .field(
                "websocket_connect_timeout_ms",
                &self.websocket_connect_timeout_ms,
            )
            .field("on_headers", &self.on_headers.as_ref().map(|_| "..."))
            .field(
                "on_provider_response",
                &self.on_provider_response.as_ref().map(|_| "..."),
            )
            .field("on_payload", &self.on_payload.as_ref().map(|_| "..."))
            .finish()
    }
}

impl Clone for StreamOptions {
    fn clone(&self) -> Self {
        Self {
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            signal: self.signal.clone(),
            api_key: self.api_key.clone(),
            transport: self.transport.clone(),
            cache_retention: self.cache_retention.clone(),
            session_id: self.session_id.clone(),
            headers: self.headers.clone(),
            timeout_ms: self.timeout_ms,
            websocket_connect_timeout_ms: self.websocket_connect_timeout_ms,
            max_retries: self.max_retries,
            max_retry_delay_ms: self.max_retry_delay_ms,
            metadata: self.metadata.clone(),
            tool_choice: self.tool_choice.clone(),
            service_tier: self.service_tier.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            thinking_budgets: self.thinking_budgets.clone(),
            debug: self.debug,
            on_payload: self.on_payload.clone(),
            on_headers: self.on_headers.clone(),
            on_provider_response: self.on_provider_response.clone(),
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            signal: None,
            api_key: None,
            transport: None,
            cache_retention: None,
            session_id: None,
            headers: None,
            timeout_ms: None,
            websocket_connect_timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            metadata: None,
            tool_choice: None,
            service_tier: None,
            reasoning_effort: None,
            thinking_budgets: None,
            debug: None,
            on_payload: None,
            on_headers: None,
            on_provider_response: None,
        }
    }
}

impl serde::Serialize for StreamOptions {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("StreamOptions", 14)?;
        if let Some(ref v) = self.temperature {
            s.serialize_field("temperature", v)?;
        }
        if let Some(ref v) = self.max_tokens {
            s.serialize_field("maxTokens", v)?;
        }
        if let Some(ref v) = self.api_key {
            s.serialize_field("apiKey", v)?;
        }
        if let Some(ref v) = self.transport {
            s.serialize_field("transport", v)?;
        }
        if let Some(ref v) = self.cache_retention {
            s.serialize_field("cacheRetention", v)?;
        }
        if let Some(ref v) = self.session_id {
            s.serialize_field("sessionId", v)?;
        }
        if let Some(ref v) = self.headers {
            s.serialize_field("headers", v)?;
        }
        if let Some(ref v) = self.timeout_ms {
            s.serialize_field("timeoutMs", v)?;
        }
        if let Some(ref v) = self.websocket_connect_timeout_ms {
            s.serialize_field("websocketConnectTimeoutMs", v)?;
        }
        if let Some(ref v) = self.max_retries {
            s.serialize_field("maxRetries", v)?;
        }
        if let Some(ref v) = self.max_retry_delay_ms {
            s.serialize_field("maxRetryDelayMs", v)?;
        }
        if let Some(ref v) = self.metadata {
            s.serialize_field("metadata", v)?;
        }
        if let Some(ref v) = self.tool_choice {
            s.serialize_field("toolChoice", v)?;
        }
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for StreamOptions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct StreamOptionsHelper {
            temperature: Option<f64>,
            max_tokens: Option<u64>,
            api_key: Option<String>,
            transport: Option<crate::types::Transport>,
            cache_retention: Option<crate::types::CacheRetention>,
            session_id: Option<String>,
            headers: Option<std::collections::HashMap<String, String>>,
            timeout_ms: Option<u64>,
            websocket_connect_timeout_ms: Option<u64>,
            max_retries: Option<u32>,
            max_retry_delay_ms: Option<u64>,
            metadata: Option<serde_json::Value>,
            tool_choice: Option<crate::types::ToolChoice>,
        }
        let helper = StreamOptionsHelper::deserialize(deserializer)?;
        Ok(Self {
            temperature: helper.temperature,
            max_tokens: helper.max_tokens,
            signal: None,
            api_key: helper.api_key,
            transport: helper.transport,
            cache_retention: helper.cache_retention,
            session_id: helper.session_id,
            headers: helper.headers,
            timeout_ms: helper.timeout_ms,
            websocket_connect_timeout_ms: helper.websocket_connect_timeout_ms,
            max_retries: helper.max_retries,
            max_retry_delay_ms: helper.max_retry_delay_ms,
            metadata: helper.metadata,
            tool_choice: helper.tool_choice,
            service_tier: None,
            reasoning_effort: None,
            thinking_budgets: None,
            debug: None,
            on_payload: None,
            on_headers: None,
            on_provider_response: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SimpleStreamOptions {
    #[serde(flatten)]
    pub base: StreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "thinkingBudgets")]
    pub thinking_budgets: Option<ThinkingBudgets>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
}

// ============================================================================
// Provider response
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub status: u16,
    pub headers: std::collections::HashMap<String, String>,
}

// ============================================================================
// Image types (minimal)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImagesModel {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub input: Vec<String>,
    pub output: Vec<String>,
    pub cost: ModelCost,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::HashMap<String, String>>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn test_content_block_text() {
        let block = ContentBlock::text("hello");
        match block {
            ContentBlock::Text { text, .. } => assert_eq!(text, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn test_tool_call_new() {
        let tc = ToolCall::new(
            "id1".into(),
            "test_tool".into(),
            serde_json::json!({"key": "value"}),
        );
        assert_eq!(tc.id, "id1");
        assert_eq!(tc.name, "test_tool");
        assert_eq!(tc.type_field, "toolCall");
    }

    #[test]
    fn test_usage_default() {
        let usage = Usage::default();
        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert!((usage.cost.total - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_message_serialization_user() {
        let msg = Message::User {
            content: vec![ContentBlock::text("hi")],
            timestamp: 123_456,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"text\":\"hi\""));
    }

    #[test]
    fn test_message_serialization_assistant() {
        let msg = Message::Assistant {
            content: vec![ContentBlock::text("hello")],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-4o".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 123_456,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"assistant\""));
        assert!(json.contains("\"stopReason\":\"stop\""));
    }

    #[test]
    fn test_message_serialization_tool_result() {
        let msg = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "test".into(),
            content: vec![ContentBlock::text("result")],
            details: Some(serde_json::json!({"status": "ok"})),
            is_error: false,
            added_tool_names: None,
            timestamp: 123_456,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"toolResult\""));
    }

    #[test]
    fn test_assistant_message_event_done() {
        let event = AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: AssistantMessage {
                content: vec![],
                api: "openai-completions".into(),
                provider: "openai".into(),
                model: "gpt-4o".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                raw_stop_reason: None,
                timestamp: 0,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"done\""));
    }

    #[test]
    fn test_stop_reason_serialization() {
        assert_eq!(
            serde_json::to_string(&StopReason::ToolUse).unwrap(),
            "\"toolUse\""
        );
        assert_eq!(
            serde_json::to_string(&StopReason::Stop).unwrap(),
            "\"stop\""
        );
    }

    #[test]
    fn test_model_serialization() {
        let model = Model {
            id: "claude-sonnet-4-6".into(),
            name: "Claude Sonnet 4.6".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec!["text".into(), "image".into()],
            cost: ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 6.0,
                tiers: vec![],
            },
            context_window: 200_000,
            max_tokens: 8192,
            headers: None,
            compat: None,
        };
        let json = serde_json::to_string(&model).unwrap();
        assert!(json.contains("\"id\":\"claude-sonnet-4-6\""));
        assert!(json.contains("\"contextWindow\":200000"));
    }

    // --- Supplementary tests matching TS originals ---

    #[test]
    fn test_content_block_text_roundtrip() {
        let block = ContentBlock::Text {
            text: "hello world".into(),
            text_signature: Some("sig123".into()),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        match parsed {
            ContentBlock::Text {
                text,
                text_signature,
            } => {
                assert_eq!(text, "hello world");
                assert_eq!(text_signature, Some("sig123".into()));
            }
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn test_content_block_tool_call_roundtrip() {
        let block = ContentBlock::ToolCall {
            id: "tc_1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
            thought_signature: None,
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        match parsed {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                assert_eq!(id, "tc_1");
                assert_eq!(name, "read_file");
                assert_eq!(arguments, serde_json::json!({"path": "/tmp/test.txt"}));
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn test_content_block_thinking_roundtrip() {
        let block = ContentBlock::Thinking {
            thinking: "Let me think...".into(),
            thinking_signature: Some("think_sig".into()),
            redacted: Some(false),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        match parsed {
            ContentBlock::Thinking {
                thinking,
                thinking_signature,
                redacted,
            } => {
                assert_eq!(thinking, "Let me think...");
                assert_eq!(thinking_signature, Some("think_sig".into()));
                assert_eq!(redacted, Some(false));
            }
            _ => panic!("expected Thinking"),
        }
    }

    #[test]
    fn test_content_block_image_roundtrip() {
        let block = ContentBlock::Image {
            data: "base64data".into(),
            mime_type: "image/png".into(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"image\""));
        assert!(json.contains("\"mimeType\":\"image/png\""));
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        match parsed {
            ContentBlock::Image { data, mime_type } => {
                assert_eq!(data, "base64data");
                assert_eq!(mime_type, "image/png");
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn test_tool_call_serialization() {
        let tc = ToolCall::new(
            "call_1".into(),
            "my_tool".into(),
            serde_json::json!({"arg": 1}),
        );
        let json = serde_json::to_string(&tc).unwrap();
        assert!(json.contains("\"type\":\"toolCall\""));
        assert!(json.contains("\"id\":\"call_1\""));
        assert!(json.contains("\"name\":\"my_tool\""));
    }

    #[test]
    fn test_usage_with_all_fields() {
        let usage = Usage {
            input: 1000,
            output: 500,
            cache_read: 200,
            cache_write: 100,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 1800,
            cost: UsageCost {
                input: 0.003,
                output: 0.0075,
                cache_read: 0.0003,
                cache_write: 0.003,
                total: 0.0138,
            },
        };
        let json = serde_json::to_string(&usage).unwrap();
        assert!(json.contains("\"input\":1000"));
        assert!(json.contains("\"output\":500"));
        assert!(json.contains("\"cacheRead\":200"));
        assert!(json.contains("\"cacheWrite\":100"));
        assert!(json.contains("\"totalTokens\":1800"));
    }

    #[test]
    fn test_assistant_message_event_start_roundtrip() {
        let msg = AssistantMessage {
            content: vec![ContentBlock::text("hi")],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-4o".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 1_234_567_890,
        };
        let event = AssistantMessageEvent::Start { partial: msg };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"start\""));
        let parsed: AssistantMessageEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            AssistantMessageEvent::Start { partial } => {
                assert_eq!(partial.model, "gpt-4o");
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn test_assistant_message_event_error_roundtrip() {
        let msg = AssistantMessage {
            content: vec![],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Error,
            error_message: Some("prompt is too long".into()),
            raw_stop_reason: None,
            timestamp: 1_234_567_890,
        };
        let event = AssistantMessageEvent::Error {
            reason: StopReason::Error,
            error: msg,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"error\""));
        let parsed: AssistantMessageEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, StopReason::Error);
                assert_eq!(error.error_message, Some("prompt is too long".into()));
            }
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn test_stop_reason_all_variants_serialize() {
        let variants = vec![
            (StopReason::Stop, "\"stop\""),
            (StopReason::Length, "\"length\""),
            (StopReason::ToolUse, "\"toolUse\""),
            (StopReason::Error, "\"error\""),
            (StopReason::Aborted, "\"aborted\""),
        ];
        for (variant, expected) in variants {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
        }
    }

    #[test]
    fn test_message_deserialize_user_with_multiple_content_blocks() {
        let json = r#"{
            "role": "user",
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "text", "text": "world"}
            ],
            "timestamp": 123456
        }"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        match msg {
            Message::User { content, timestamp } => {
                assert_eq!(content.len(), 2);
                assert_eq!(timestamp, 123456);
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn test_model_with_compat_openai() {
        let model = Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: "openai-completions".into(),
            provider: "openai".into(),
            base_url: "https://api.openai.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into(), "image".into()],
            cost: ModelCost {
                input: 2.5,
                output: 10.0,
                cache_read: 1.25,
                cache_write: 0.0,
                tiers: vec![],
            },
            context_window: 128_000,
            max_tokens: 16_384,
            headers: None,
            compat: Some(ModelCompat::OpenAICompletions(Box::new(
                OpenAICompletionsCompat {
                    supports_store: Some(true),
                    max_tokens_field: Some("max_completion_tokens".into()),
                    ..Default::default()
                },
            ))),
        };
        let json = serde_json::to_string(&model).unwrap();
        assert!(json.contains("\"supportsStore\":true"));
        assert!(json.contains("\"maxTokensField\":\"max_completion_tokens\""));
    }

    #[test]
    fn test_stream_options_default() {
        let opts = StreamOptions::default();
        assert!(opts.temperature.is_none());
        assert!(opts.max_tokens.is_none());
        assert!(opts.api_key.is_none());
    }

    #[test]
    fn test_context_serialization() {
        let ctx = Context {
            system_prompt: Some("You are helpful.".into()),
            messages: vec![Message::User {
                content: vec![ContentBlock::text("hi")],
                timestamp: 0,
            }],
            tools: Some(vec![Tool {
                name: "echo".into(),
                description: "Echoes input".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                constrained_sampling: None,
            }]),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"systemPrompt\":\"You are helpful.\""));
        assert!(json.contains("\"tools\""));
    }
}
