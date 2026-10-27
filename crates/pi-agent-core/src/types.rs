use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::pi_ai_types::{
    AssistantMessageEvent, ContentBlock, Message, Model, StopReason, ThinkingLevel,
    ToolExecutionMode, Usage,
};

/// TS `AgentMessage` wire shape: `role`-tagged union with camelCase fields
/// (`stopReason`, `toolCallId`, `isError`, `exitCode`, `customType`, …).
/// The snake_case aliases keep session files written by older pi-rs builds
/// readable, and the TS shape is what `--mode json` / RPC emit and what
/// original-pi session files contain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all_fields = "camelCase")]
pub enum AgentMessage {
    /// System instructions and tool declarations at one point in the transcript
    /// (TS `SystemMessage` in `AgentMessage`). Replayed into the provider
    /// prompt/tools by the transcript helpers.
    #[serde(rename = "system")]
    System {
        content: crate::pi_ai_types::SystemContent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sections: Option<indexmap::IndexMap<String, Option<String>>>,
        #[serde(
            rename = "toolsAdded",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        tools_added: Option<Vec<crate::pi_ai_types::Tool>>,
        #[serde(
            rename = "toolsRemoved",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        tools_removed: Option<Vec<crate::pi_ai_types::ToolReference>>,
        timestamp: i64,
    },
    #[serde(rename = "user")]
    User {
        /// TS `UserMessage.content` is `string | (Text|Image)[]`; accepting the
        /// shorthand keeps original-pi session files and hand-written fixtures
        /// loadable (they simply become a single text block).
        #[serde(deserialize_with = "deserialize_user_content")]
        content: Vec<ContentBlock>,
        timestamp: i64,
    },
    #[serde(rename = "assistant")]
    Assistant {
        content: Vec<ContentBlock>,
        api: String,
        provider: String,
        model: String,
        usage: Usage,
        #[serde(alias = "stop_reason")]
        stop_reason: Option<StopReason>,
        #[serde(alias = "error_message")]
        error_message: Option<String>,
        timestamp: i64,
    },
    #[serde(rename = "toolResult")]
    ToolResult {
        #[serde(alias = "tool_call_id")]
        tool_call_id: String,
        #[serde(alias = "tool_name")]
        tool_name: String,
        content: Vec<ContentBlock>,
        details: serde_json::Value,
        #[serde(alias = "is_error")]
        is_error: bool,
        /// Usage from the tool execution itself, if available (match TS
        /// `ToolResultMessage.usage`; not used for main LLM context accounting).
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        timestamp: i64,
    },
    #[serde(rename = "bashExecution")]
    BashExecution {
        command: String,
        output: String,
        #[serde(alias = "exit_code")]
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        #[serde(alias = "full_output_path")]
        full_output_path: Option<String>,
        timestamp: i64,
        #[serde(alias = "exclude_from_context")]
        exclude_from_context: Option<bool>,
    },
    #[serde(rename = "custom")]
    Custom {
        #[serde(alias = "custom_type")]
        custom_type: String,
        content: CustomContent,
        display: bool,
        details: Option<serde_json::Value>,
        timestamp: i64,
    },
    #[serde(rename = "branchSummary")]
    BranchSummary {
        summary: String,
        #[serde(alias = "from_id")]
        from_id: String,
        timestamp: i64,
    },
    #[serde(rename = "compactionSummary")]
    CompactionSummary {
        summary: String,
        #[serde(alias = "tokens_before")]
        tokens_before: u64,
        timestamp: i64,
    },
}

/// Accept TS's `content: string` shorthand for user messages in addition to
/// the canonical content-block array.
fn deserialize_user_content<'de, D>(deserializer: D) -> Result<Vec<ContentBlock>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Text(String),
        Blocks(Vec<ContentBlock>),
    }
    Ok(match Repr::deserialize(deserializer)? {
        Repr::Text(text) => vec![ContentBlock::Text {
            text,
            text_signature: None,
        }],
        Repr::Blocks(blocks) => blocks,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum CustomContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl AgentMessage {
    pub fn role(&self) -> &str {
        match self {
            AgentMessage::System { .. } => "system",
            AgentMessage::User { .. } => "user",
            AgentMessage::Assistant { .. } => "assistant",
            AgentMessage::ToolResult { .. } => "toolResult",
            AgentMessage::BashExecution { .. } => "bashExecution",
            AgentMessage::Custom { .. } => "custom",
            AgentMessage::BranchSummary { .. } => "branchSummary",
            AgentMessage::CompactionSummary { .. } => "compactionSummary",
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            AgentMessage::System { timestamp, .. } => *timestamp,
            AgentMessage::User { timestamp, .. } => *timestamp,
            AgentMessage::Assistant { timestamp, .. } => *timestamp,
            AgentMessage::ToolResult { timestamp, .. } => *timestamp,
            AgentMessage::BashExecution { timestamp, .. } => *timestamp,
            AgentMessage::Custom { timestamp, .. } => *timestamp,
            AgentMessage::BranchSummary { timestamp, .. } => *timestamp,
            AgentMessage::CompactionSummary { timestamp, .. } => *timestamp,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolResult<T: Clone + Send + Sync + 'static> {
    pub content: Vec<ContentBlock>,
    pub details: T,
    /// Usage from the final tool execution itself, if available.
    /// Not used for main LLM context accounting (match TS `AgentToolResult.usage`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

/// Error wrapper for a tool that failed but still has a structured result to
/// expose to the model (for example an extension tool returning
/// `ToolCallOutput { is_error: true, .. }`).
///
/// The agent loop unwraps this type and preserves `result` while marking the
/// final tool call as an error. Ordinary errors still use the generic error
/// result path.
#[derive(Debug, Clone)]
pub struct ToolExecutionError {
    /// Content and details produced by the failed tool.
    pub result: AgentToolResult<serde_json::Value>,
    /// Human-readable summary used if a caller cannot consume `result`.
    pub message: String,
}

impl ToolExecutionError {
    /// Create a structured tool failure.
    #[must_use]
    pub fn new(result: AgentToolResult<serde_json::Value>, message: impl Into<String>) -> Self {
        Self {
            result,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ToolExecutionError {}

#[derive(Debug, Clone)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

pub type DynTool = AgentTool<serde_json::Value, serde_json::Value>;

pub type AgentToolUpdateCallback<T> = Arc<dyn Fn(AgentToolResult<T>) + Send + Sync>;

#[allow(clippy::type_complexity)]
pub struct AgentTool<TParams, TDetails>
where
    TParams: Clone + Send + Sync + 'static,
    TDetails: Clone + Send + Sync + 'static,
{
    pub name: String,
    pub description: String,
    pub label: String,
    /// Optional provider-side constrained sampling config (match TS
    /// `Tool.constrainedSampling`). `None` = not set; providers without
    /// strict-tool support fall back to plain schema sampling.
    pub constrained_sampling: Option<crate::pi_ai_types::ConstrainedSamplingConfig>,
    /// Short system-prompt contribution for this tool (matching TS
    /// `ToolDefinition.promptSnippet`), injected into the system prompt so the
    /// model knows what each enabled tool is for.
    pub prompt_snippet: Option<String>,
    /// Additional system-prompt guidelines for this tool (matching TS
    /// `ToolDefinition.promptGuidelines`).
    pub prompt_guidelines: Option<Vec<String>>,
    pub parameters_schema: serde_json::Value,
    pub execution_mode: Option<ToolExecutionMode>,
    pub prepare_arguments: Option<Arc<dyn Fn(&serde_json::Value) -> TParams + Send + Sync>>,
    pub execute: Arc<
        dyn Fn(
                String,
                TParams,
                Option<tokio::sync::watch::Receiver<bool>>,
                Option<Arc<dyn Fn(AgentToolResult<TDetails>) + Send + Sync>>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<
                                AgentToolResult<TDetails>,
                                Box<dyn std::error::Error + Send + Sync>,
                            >,
                        > + Send,
                >,
            > + Send
            + Sync,
    >,
}

impl<TParams, TDetails> std::fmt::Debug for AgentTool<TParams, TDetails>
where
    TParams: Clone + Send + Sync + 'static,
    TDetails: Clone + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("label", &self.label)
            .finish()
    }
}

impl<TParams, TDetails> Clone for AgentTool<TParams, TDetails>
where
    TParams: Clone + Send + Sync + 'static,
    TDetails: Clone + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            description: self.description.clone(),
            label: self.label.clone(),
            prompt_snippet: self.prompt_snippet.clone(),
            prompt_guidelines: self.prompt_guidelines.clone(),
            parameters_schema: self.parameters_schema.clone(),
            constrained_sampling: self.constrained_sampling.clone(),
            execution_mode: self.execution_mode,
            prepare_arguments: self.prepare_arguments.clone(),
            execute: self.execute.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Option<Vec<Arc<DynTool>>>,
}

impl std::fmt::Debug for AgentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentContext")
            .field("system_prompt_len", &self.system_prompt.len())
            .field("messages_count", &self.messages.len())
            .field("tools_count", &self.tools.as_ref().map(|t| t.len()))
            .finish()
    }
}

#[derive(Clone)]
pub struct AgentState {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub tools: Vec<Arc<DynTool>>,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: HashSet<String>,
    pub error_message: Option<String>,
}

impl std::fmt::Debug for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentState")
            .field("system_prompt", &self.system_prompt.len())
            .field("model", &self.model)
            .field("thinking_level", &self.thinking_level)
            .field("tools_count", &self.tools.len())
            .field("messages_count", &self.messages.len())
            .field("is_streaming", &self.is_streaming)
            .field("pending_tool_calls", &self.pending_tool_calls)
            .field("error_message", &self.error_message)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    TurnStart,
    TurnEnd {
        message: AgentMessage,
        tool_results: Vec<AgentMessage>,
    },
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        message: AgentMessage,
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        partial_result: serde_json::Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: serde_json::Value,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum QueueMode {
    All,
    OneAtATime,
}

#[derive(Debug, Clone)]
pub struct BeforeToolCallResult {
    pub block: bool,
    pub reason: Option<String>,
    /// Modified tool arguments from extension hooks.
    /// When `Some(args)`, the tool call will use these args instead of the original.
    pub modified_args: Option<serde_json::Value>,
    /// TS 0.84.1 (#7715): hint that the agent should stop after the current
    /// tool batch when this call is blocked. Early termination only happens
    /// when every finalized tool result in the batch sets this to true.
    pub terminate: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct AfterToolCallResult {
    pub content: Option<Vec<ContentBlock>>,
    pub details: Option<serde_json::Value>,
    pub is_error: Option<bool>,
    pub terminate: Option<bool>,
}

#[derive(Clone)]
pub struct BeforeToolCallContext {
    pub assistant_message: AgentMessage,
    pub tool_call: AgentToolCall,
    pub args: serde_json::Value,
    pub context: AgentContext,
}

pub struct AfterToolCallContext {
    pub assistant_message: AgentMessage,
    pub tool_call: AgentToolCall,
    pub args: serde_json::Value,
    pub result: AgentToolResult<serde_json::Value>,
    pub is_error: bool,
    pub context: AgentContext,
}

/// Context passed to completed-turn callbacks (TS `AgentTurnContext`).
#[derive(Clone)]
pub struct AgentTurnContext {
    pub message: AgentMessage,
    pub tool_results: Vec<AgentMessage>,
    pub context: AgentContext,
    pub new_messages: Vec<AgentMessage>,
}

/// Decision returned by [`FinishTurnFn`] (TS `AgentTurnDecision`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentTurnDecision {
    /// Ensure one next provider request (no-op when tool results / steering /
    /// follow-up already schedule one).
    Continue,
    /// End the run without polling queues or preparing another request.
    End,
}

/// Alias to match TS `PrepareNextTurnContext extends AgentTurnContext`.
pub type PrepareNextTurnContext = AgentTurnContext;

/// Runtime state available immediately before a conversational provider
/// request (TS `PrepareRequestContext`).
#[derive(Clone)]
pub struct PrepareRequestContext {
    pub context: AgentContext,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
}

/// Replacement runtime state for the provider request being prepared
/// (TS `AgentRequestUpdate` = `Omit<AgentLoopTurnUpdate, "messages">`).
#[derive(Clone, Default)]
pub struct AgentRequestUpdate {
    pub context: Option<AgentContext>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
}

/// What `prepareNextTurn` may change for the next provider request (TS
/// `AgentLoopTurnUpdate`).
#[derive(Default)]
pub struct AgentLoopTurnUpdate {
    /// Context for the next provider request.
    pub context: Option<AgentContext>,
    /// Messages to append before the next provider request, with the normal
    /// message lifecycle events (TS `messages`).
    pub messages: Option<Vec<AgentMessage>>,
    /// Model for the next provider request.
    pub model: Option<Model>,
    /// Thinking level for the next provider request.
    pub thinking_level: Option<ThinkingLevel>,
}

pub type StreamFn = Arc<
    dyn Fn(
            Model,
            crate::pi_ai_types::Context,
            Option<crate::pi_ai_types::ThinkingLevel>,
            StreamFnOptions,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            crate::pi_ai_types::StreamResponse,
                            Box<dyn std::error::Error + Send + Sync>,
                        >,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

#[derive(Clone, Default)]
#[allow(clippy::type_complexity)]
pub struct StreamFnOptions {
    pub api_key: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub session_id: Option<String>,
    pub thinking_budgets: Option<crate::pi_ai_types::ThinkingBudgets>,
    pub max_retry_delay_ms: Option<u64>,
    pub transport: Option<String>,
    /// Pass-through of `StreamOptions` fields (match TS `SimpleStreamOptions`
    /// spread into the LLM call).
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub websocket_connect_timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub cache_retention: Option<crate::pi_ai_types::CacheRetention>,
    pub tool_choice: Option<crate::pi_ai_types::ToolChoice>,
    pub service_tier: Option<String>,
    pub metadata: Option<serde_json::Value>,
    /// Provider-scoped environment overlay (match TS `SimpleStreamOptions.env`).
    pub env: Option<std::collections::HashMap<String, String>>,
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
}

impl std::fmt::Debug for StreamFnOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamFnOptions")
            .field("api_key", &self.api_key.as_ref().map(|_| "..."))
            .field("session_id", &self.session_id)
            .field("transport", &self.transport)
            .finish()
    }
}

pub type AgentEventSink = Arc<
    dyn Fn(AgentEvent) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

pub type ConvertToLlmFn = Arc<dyn Fn(&[AgentMessage]) -> Vec<Message> + Send + Sync>;

pub type TransformContextFn = Arc<
    dyn Fn(
            Vec<AgentMessage>,
            Option<tokio::sync::watch::Receiver<bool>>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<AgentMessage>> + Send>>
        + Send
        + Sync,
>;

pub type GetApiKeyFn = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>
        + Send
        + Sync,
>;

pub type GetSteeringMessagesFn = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<AgentMessage>> + Send>>
        + Send
        + Sync,
>;

pub type GetFollowUpMessagesFn = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<AgentMessage>> + Send>>
        + Send
        + Sync,
>;

pub type FinishTurnFn = Arc<
    dyn Fn(
            AgentTurnContext,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<AgentTurnDecision>> + Send>,
        > + Send
        + Sync,
>;

/// Matches TS `AgentLoopConfig.prepareRequest` — called before every provider
/// request, including the first.
pub type PrepareRequestFn = Arc<
    dyn Fn(
            PrepareRequestContext,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<AgentRequestUpdate>> + Send>,
        > + Send
        + Sync,
>;

/// Matches TS `AgentOptions.prepareNextTurn` — receives an optional abort signal,
/// NOT the turn context. The agent loop wraps this internally.
pub type PrepareNextTurnOptionsFn = Arc<
    dyn Fn(
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<AgentLoopTurnUpdate>> + Send>,
        > + Send
        + Sync,
>;

/// Matches TS `AgentLoopConfig.prepareNextTurn` — receives the full turn context
/// and an optional abort signal.
pub type PrepareNextTurnFn = Arc<
    dyn Fn(
            AgentTurnContext,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<AgentLoopTurnUpdate>> + Send>,
        > + Send
        + Sync,
>;

pub type BeforeToolCallFn = Arc<
    dyn Fn(
            BeforeToolCallContext,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<BeforeToolCallResult>> + Send>,
        > + Send
        + Sync,
>;

pub type AfterToolCallFn = Arc<
    dyn Fn(
            AfterToolCallContext,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<AfterToolCallResult>> + Send>,
        > + Send
        + Sync,
>;

#[cfg(test)]
mod wire_format_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// `AgentMessage` must serialize with the TS wire shape (camelCase), which
    /// is what `--mode json`, RPC mode and original-pi session files use.
    /// Consumers (pi-acp, subagent) read `stopReason` / `toolCallId` / `isError`.
    #[test]
    fn agent_message_serializes_camel_case() {
        let assistant = AgentMessage::Assistant {
            content: vec![ContentBlock::Text {
                text: "hi".into(),
                text_signature: Some("sig".into()),
            }],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            usage: Usage::default(),
            stop_reason: Some(StopReason::Stop),
            error_message: None,
            timestamp: 1,
        };
        let v = serde_json::to_value(&assistant).unwrap();
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["stopReason"], "stop");
        assert!(v.get("stop_reason").is_none(), "no snake_case on the wire");
        assert_eq!(v["content"][0]["textSignature"], "sig");

        let tool_result = AgentMessage::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "bash".into(),
            content: vec![],
            details: serde_json::json!({}),
            is_error: true,
            usage: None,
            timestamp: 2,
        };
        let v = serde_json::to_value(&tool_result).unwrap();
        assert_eq!(v["role"], "toolResult");
        assert_eq!(v["toolCallId"], "tc1");
        assert_eq!(v["toolName"], "bash");
        assert_eq!(v["isError"], true);
    }

    /// Session files written by older pi-rs builds used snake_case; they must
    /// still load (serde aliases).
    #[test]
    fn agent_message_accepts_legacy_snake_case() {
        let legacy = serde_json::json!({
            "role": "toolResult",
            "tool_call_id": "tc1",
            "tool_name": "bash",
            "content": [],
            "details": {},
            "is_error": false,
            "timestamp": 1
        });
        let msg: AgentMessage = serde_json::from_value(legacy).unwrap();
        match msg {
            AgentMessage::ToolResult { tool_call_id, tool_name, is_error, .. } => {
                assert_eq!(tool_call_id, "tc1");
                assert_eq!(tool_name, "bash");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}
