use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::pi_ai_types::{
    AssistantMessageEvent, ContentBlock, Message, Model, StopReason, ThinkingLevel,
    ToolExecutionMode, Usage,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role")]
pub enum AgentMessage {
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
        usage: Usage,
        stop_reason: Option<StopReason>,
        error_message: Option<String>,
        timestamp: i64,
    },
    #[serde(rename = "toolResult")]
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<ContentBlock>,
        details: serde_json::Value,
        is_error: bool,
        timestamp: i64,
    },
    #[serde(rename = "bashExecution")]
    BashExecution {
        command: String,
        output: String,
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
        timestamp: i64,
        exclude_from_context: Option<bool>,
    },
    #[serde(rename = "custom")]
    Custom {
        custom_type: String,
        content: CustomContent,
        display: bool,
        details: Option<serde_json::Value>,
        timestamp: i64,
    },
    #[serde(rename = "branchSummary")]
    BranchSummary {
        summary: String,
        from_id: String,
        timestamp: i64,
    },
    #[serde(rename = "compactionSummary")]
    CompactionSummary {
        summary: String,
        tokens_before: u64,
        timestamp: i64,
    },
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

pub type DynTool = AgentTool<serde_json::Value, serde_json::Value>;

pub type AgentToolUpdateCallback<T> = Arc<dyn Fn(AgentToolResult<T>) + Send + Sync>;

pub struct AgentTool<TParams, TDetails>
where
    TParams: Clone + Send + Sync + 'static,
    TDetails: Clone + Send + Sync + 'static,
{
    pub name: String,
    pub description: String,
    pub label: String,
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
            parameters_schema: self.parameters_schema.clone(),
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
}

#[derive(Debug, Clone)]
pub struct AfterToolCallResult {
    pub content: Option<Vec<ContentBlock>>,
    pub details: Option<serde_json::Value>,
    pub is_error: Option<bool>,
    pub terminate: Option<bool>,
}

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

#[derive(Clone)]
pub struct ShouldStopAfterTurnContext {
    pub message: AgentMessage,
    pub tool_results: Vec<AgentMessage>,
    pub context: AgentContext,
    pub new_messages: Vec<AgentMessage>,
}

/// Alias to match TS `PrepareNextTurnContext extends ShouldStopAfterTurnContext`.
pub type PrepareNextTurnContext = ShouldStopAfterTurnContext;

pub struct AgentLoopTurnUpdate {
    pub context: Option<AgentContext>,
    pub model: Option<Model>,
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
pub struct StreamFnOptions {
    pub api_key: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub session_id: Option<String>,
    pub thinking_budgets: Option<crate::pi_ai_types::ThinkingBudgets>,
    pub max_retry_delay_ms: Option<u64>,
    pub transport: Option<String>,
    pub on_payload: Option<Arc<dyn Fn(serde_json::Value) + Send + Sync>>,
    pub on_response: Option<Arc<dyn Fn(&crate::pi_ai_types::AssistantMessage) + Send + Sync>>,
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

pub type ShouldStopAfterTurnFn = Arc<
    dyn Fn(
            ShouldStopAfterTurnContext,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
        + Send
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
            ShouldStopAfterTurnContext,
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
