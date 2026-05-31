use serde::{Deserialize, Serialize};

// ============================================================================
// Content block types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
        ContentBlock::Text {
            text: text.into(),
            text_signature: None,
        }
    }
}

// ============================================================================
// ToolCall (standalone, used in events)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl Default for UsageCost {
    fn default() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    #[serde(rename = "totalTokens")]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: UsageCost,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            total_tokens: 0,
            cost: UsageCost::default(),
        }
    }
}

// ============================================================================
// Stop reason
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
pub struct AssistantMessageDiagnostic {
    #[serde(rename = "contentIndex")]
    pub content_index: usize,
    pub diagnostic: String,
    pub severity: DiagnosticSeverity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Warning,
    Error,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIResponsesCompat {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_session_id_header: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ModelCompat {
    OpenAICompletions(OpenAICompletionsCompat),
    OpenAIResponses(OpenAIResponsesCompat),
    AnthropicMessages(AnthropicMessagesCompat),
}

// ============================================================================
// Tool types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip)]
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "cacheRetention")]
    pub cache_retention: Option<CacheRetention>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "timeoutMs")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "websocketConnectTimeoutMs")]
    pub websocket_connect_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maxRetries")]
    pub max_retries: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maxRetryDelayMs")]
    pub max_retry_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
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
        let tc = ToolCall::new("id1".into(), "test_tool".into(), serde_json::json!({"key": "value"}));
        assert_eq!(tc.id, "id1");
        assert_eq!(tc.name, "test_tool");
        assert_eq!(tc.type_field, "toolCall");
    }

    #[test]
    fn test_usage_default() {
        let usage = Usage::default();
        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert_eq!(usage.cost.total, 0.0);
    }

    #[test]
    fn test_message_serialization_user() {
        let msg = Message::User {
            content: vec![ContentBlock::text("hi")],
            timestamp: 123456,
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
            timestamp: 123456,
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
            timestamp: 123456,
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
            },
            context_window: 200000,
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
            ContentBlock::Text { text, text_signature } => {
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
            ContentBlock::ToolCall { id, name, arguments, .. } => {
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
            ContentBlock::Thinking { thinking, thinking_signature, redacted } => {
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
        let tc = ToolCall::new("call_1".into(), "my_tool".into(), serde_json::json!({"arg": 1}));
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
            timestamp: 1234567890,
        };
        let event = AssistantMessageEvent::Start { partial: msg.clone() };
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
        let mut msg = AssistantMessage {
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
            timestamp: 1234567890,
        };
        let event = AssistantMessageEvent::Error {
            reason: StopReason::Error,
            error: msg.clone(),
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
            cost: ModelCost { input: 2.5, output: 10.0, cache_read: 1.25, cache_write: 0.0 },
            context_window: 128000,
            max_tokens: 16384,
            headers: None,
            compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
                supports_store: Some(true),
                max_tokens_field: Some("max_completion_tokens".into()),
                ..Default::default()
            })),
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
            }]),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"systemPrompt\":\"You are helpful.\""));
        assert!(json.contains("\"tools\""));
    }
}
