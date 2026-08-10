use pi_agent_core::agent::Agent;
use pi_agent_core::pi_ai_types::{ContentBlock, Model, ThinkingLevel};
use pi_agent_core::types::{
    AfterToolCallFn, AgentEvent, AgentMessage, AgentState, BeforeToolCallFn, ConvertToLlmFn,
    QueueMode, StreamFn, TransformContextFn,
};
use std::sync::Arc;

use crate::core::compaction::CompactionResult;
use crate::core::compaction::CompactionSettings;
use crate::core::context_usage::ContextUsage;
use crate::core::extensions::{ExtensionContext, ExtensionRegistry, ToolDefinition};
use crate::core::messages;
use crate::core::model_registry::ModelRegistry;
use crate::core::resource_loader::LoadedResources;
use crate::core::session_manager::SessionEntry;
use crate::core::session_manager::SessionManager;
use crate::core::system_prompt::{self, ContextFile, SkillInfo};
use crate::core::tools;
use pi_agent_core::pi_ai_types::AssistantMessageEvent;
use tokio::sync::Notify;

// ============================================================================
// Types
// ============================================================================

/// Abort handler called when the session is aborted.
pub type AbortHandler = Box<dyn Fn() + Send + Sync>;
/// Shutdown handler called when the session is disposed.
pub type ShutdownHandler = Box<dyn Fn() + Send + Sync>;
/// Error listener for extension errors.
/// An error raised by an extension, matching TS `ExtensionError`.
#[derive(Debug, Clone)]
pub struct ExtensionError {
    /// Path of the extension that raised the error.
    pub extension_path: String,
    /// The extension event that was being handled (e.g. "tool_call").
    pub event: String,
    /// Error message.
    pub error: String,
}

/// Listener for extension errors, matching TS `bindExtensions({ onError })`.
pub type ErrorListener = Box<dyn Fn(&ExtensionError) + Send + Sync>;

/// Output-chunk callback for `execute_bash` (not an error listener).
pub type ChunkCallback = Box<dyn Fn(&str) + Send + Sync>;
/// Send a custom message (bound to this session).
/// A custom message sent by an extension, matching TS `CustomMessage`.
#[derive(Debug, Clone)]
pub struct CustomMessage {
    pub custom_type: String,
    pub content: Vec<ContentBlock>,
    pub display: Option<bool>,
    pub details: Option<serde_json::Value>,
}

/// Send a custom message (bound to this session), matching TS
/// `sendMessage(message, options)`.
pub type SendMessageFn = Box<dyn Fn(CustomMessage, Option<CustomMessageOptions>) + Send + Sync>;
/// Send a user message (bound to this session).
pub type SendUserMessageFn = Box<dyn Fn(String, Option<SendUserMessageOptions>) + Send + Sync>;
/// Extension hook: transform the provider request payload.
pub type PayloadHookFn = Arc<
    dyn Fn(
            serde_json::Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send>,
        > + Send
        + Sync,
>;
/// Extension hook: transform provider request headers.
pub type HeadersHookFn = Arc<
    dyn Fn(
            std::collections::HashMap<String, String>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = std::collections::HashMap<String, String>,
                    > + Send,
            >,
        > + Send
        + Sync,
>;
/// Extension hook: observe provider response status/headers.
pub type ProviderResponseHookFn =
    Arc<dyn Fn(u16, std::collections::HashMap<String, String>) + Send + Sync>;
/// Agent event listener: `(event, signal) -> Future<()>`.
pub type AgentEventListener = Arc<
    dyn Fn(
            AgentEvent,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;
/// Tool execute closure for custom/extension tools.
pub type ToolExecuteFn = Arc<
    dyn Fn(
            String,
            serde_json::Value,
            Option<tokio::sync::watch::Receiver<bool>>,
            Option<
                Arc<
                    dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync,
                >,
            >,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            pi_agent_core::types::AgentToolResult<serde_json::Value>,
                            Box<dyn std::error::Error + Send + Sync>,
                        >,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

/// Configuration for creating an AgentSession.
/// Matches the original TypeScript AgentSessionConfig interface.
pub struct AgentSessionConfig {
    pub cwd: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub custom_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub selected_tools: Option<Vec<String>>,
    pub tool_snippets: Option<std::collections::HashMap<String, String>>,
    pub prompt_guidelines: Option<Vec<String>>,
    pub context_files: Vec<ContextFile>,
    pub skills: Vec<SkillInfo>,
    pub session_name: Option<String>,
    pub stream_fn: Option<StreamFn>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    pub initial_active_tool_names: Option<Vec<String>>,
    pub allowed_tool_names: Option<Vec<String>>,
    pub excluded_tool_names: Option<Vec<String>>,
    /// Extension registry (Rust native extensions).
    pub extension_registry: Option<std::sync::Arc<ExtensionRegistry>>,
    /// Loaded resources (skills, extensions, prompt templates).
    pub resources: Option<LoadedResources>,
    /// Custom tool definitions injected by the caller (e.g. trading tools).
    /// The SDK creates stub DynTool entries from these definitions.
    /// Call `agent.add_tools()` after session creation to replace stubs
    /// with real execute implementations.
    pub custom_tools: Option<Vec<ToolDefinition>>,
    /// Shared state snapshot for JS extension read-actions (getActiveTools,
    /// getSessionName, ...). Refreshed at drain points.
    pub extension_state_view: Option<Arc<std::sync::Mutex<crate::core::extensions::action_bus::ExtensionStateView>>>,
    /// Receiver for JS extension write-actions (sendMessage, setModel, ...),
    /// drained at turn boundaries.
    pub extension_action_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::core::extensions::action_bus::ExtensionAction>>,
}

/// Options for AgentSession.prompt().
/// Matches the original TypeScript PromptOptions interface.
/// Options for send_custom_message(), matching TS sendCustomMessage() options.
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessageOptions {
    /// Whether to trigger a turn after sending the message.
    #[serde(default)]
    pub trigger_turn: bool,
    /// Delivery mode when streaming: "steer", "followUp", or "nextTurn".
    #[serde(default)]
    pub deliver_as: Option<String>,
}

/// Options for send_user_message(), matching TS sendUserMessage() options.
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendUserMessageOptions {
    /// Delivery mode when streaming: "steer" or "followUp".
    #[serde(default)]
    pub deliver_as: Option<String>,
}

/// Options for prompt(), matching TS PromptOptions interface.
#[derive(Default)]
pub struct PromptOptions {
    /// Whether to expand file-based prompt templates (default: true).
    pub expand_prompt_templates: Option<bool>,
    /// Image attachments.
    pub images: Option<Vec<ContentBlock>>,
    /// When streaming, how to queue the message: "steer" (interrupt) or "followUp" (wait).
    pub streaming_behavior: Option<String>,
    /// Source of input for extension input event handlers. Defaults to "interactive".
    pub source: Option<String>,
    /// Internal hook used by RPC mode to observe prompt preflight acceptance
    /// or rejection (matching TS `PromptOptions.preflightResult`). Called with
    /// `true` when the prompt is accepted (queued or about to run), `false`
    /// when a preflight check rejects it.
    pub preflight_result: Option<Arc<dyn Fn(bool) + Send + Sync>>,
}

/// Extension bindings for bind_extensions(), matching TS ExtensionBindings interface.
/// In the current Rust architecture, extensions are registered at construction time,
/// so most fields are informational. The struct is kept for API compatibility.
#[derive(Default)]
pub struct ExtensionBindings {
    /// UI context for extension notifications/confirmations.
    pub ui_context: Option<crate::core::extensions::ExtensionUIContext>,
    /// Extension mode ("tui", "rpc", "json", "print").
    pub mode: Option<String>,
    /// Abort handler called when the session is aborted.
    pub abort_handler: Option<AbortHandler>,
    /// Shutdown handler called when the session is disposed.
    pub shutdown_handler: Option<ShutdownHandler>,
    /// Error listener for extension errors.
    pub on_error: Option<ErrorListener>,
}

/// Replaced session context for create_replaced_session_context(),
/// matching TS ReplacedSessionContext interface.
pub struct ReplacedSessionContext {
    /// Send a custom message (bound to this session).
    pub send_message: Option<SendMessageFn>,
    /// Send a user message (bound to this session).
    pub send_user_message: Option<SendUserMessageFn>,
}

/// Session statistics for /session command.
/// Matches the original TypeScript SessionStats interface.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SessionStats {
    pub session_file: Option<String>,
    pub session_id: String,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub total_messages: usize,
    pub tokens: TokenUsage,
    pub cost: f64,
    #[serde(skip)]
    pub context_usage: Option<ContextUsage>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

// ============================================================================
// AgentSessionEvent — Session-level events for UI layer
// ============================================================================

/// Reason for compaction, matching TS `"manual" | "threshold" | "overflow"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

impl std::fmt::Display for CompactionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactionReason::Manual => write!(f, "manual"),
            CompactionReason::Threshold => write!(f, "threshold"),
            CompactionReason::Overflow => write!(f, "overflow"),
        }
    }
}

/// Session-specific events that extend the core AgentEvent.
/// Matches the original TypeScript AgentSessionEvent type.
///
/// Serialized as a `type`-discriminated union, matching the TS
/// `AgentSessionEvent` shape on the RPC/JSON wire (`{"type": "message_update", ...}`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum AgentSessionEvent {
    // ── Passthrough from AgentEvent (all variants except AgentEnd) ──
    AgentStart,
    TurnStart,
    #[serde(rename_all = "camelCase")]
    TurnEnd {
        message: AgentMessage,
        tool_results: Vec<AgentMessage>,
    },
    MessageStart {
        message: AgentMessage,
    },
    #[serde(rename_all = "camelCase")]
    MessageUpdate {
        message: AgentMessage,
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: AgentMessage,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        partial_result: serde_json::Value,
    },
    #[serde(rename_all = "camelCase")]
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: serde_json::Value,
        is_error: bool,
    },
    // ── AgentEnd with willRetry ──
    #[serde(rename_all = "camelCase")]
    AgentEnd {
        messages: Vec<AgentMessage>,
        will_retry: bool,
    },
    // ── Session-specific events ──
    AgentSettled,
    #[serde(rename_all = "camelCase")]
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    CompactionStart {
        reason: CompactionReason,
    },
    EntryAppended {
        entry: SessionEntry,
    },
    SessionInfoChanged {
        name: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ModelSelect {
        model: String,
        previous_model: Option<String>,
        source: String,
    },
    ThinkingLevelChanged {
        level: String,
    },
    #[serde(rename_all = "camelCase")]
    CompactionEnd {
        reason: CompactionReason,
        result: Option<CompactionResult>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error_message: String,
    },
    #[serde(rename_all = "camelCase")]
    AutoRetryEnd {
        success: bool,
        attempt: u32,
        final_error: Option<String>,
    },
    /// Emitted before the backoff sleep of a compaction/branch-summary retry
    /// (match TS `summarization_retry_scheduled`).
    #[serde(rename_all = "camelCase")]
    SummarizationRetryScheduled {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error_message: String,
    },
    /// Emitted after the backoff sleep, immediately before the retried
    /// summarization call starts (match TS `summarization_retry_attempt_start`).
    #[serde(rename_all = "camelCase")]
    SummarizationRetryAttemptStart {
        source: String,
        reason: Option<String>,
    },
    /// Emitted once when the summarization retry loop ends
    /// (match TS `summarization_retry_finished`).
    #[serde(rename_all = "camelCase")]
    SummarizationRetryFinished {},
    /// Streamed output delta from a direct bash execution, correlated with the
    /// request id (match TS `bash_execution_update`, #6971).
    #[serde(rename_all = "camelCase")]
    BashExecutionUpdate {
        id: Option<String>,
        delta: String,
    },
}

/// Listener function for agent session events.
pub type AgentSessionEventListener = Arc<dyn Fn(AgentSessionEvent) + Send + Sync>;

/// Handle returned by [`AgentSession::subscribe_session_events`].
/// Call `unsubscribe()` to stop receiving events.
pub struct SessionEventUnsubscribeHandle {
    listeners: Arc<std::sync::Mutex<Vec<AgentSessionEventListener>>>,
    index: usize,
}

impl SessionEventUnsubscribeHandle {
    /// Remove the listener. After this call the listener will no longer receive events.
    pub fn unsubscribe(self) {
        let mut listeners = self.listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.index < listeners.len() {
            // Replace with a no-op so the slot stays valid and Vec indices are not disturbed.
            listeners[self.index] = Arc::new(|_| {});
        }
    }
}

// ============================================================================
// AgentSession
// ============================================================================

#[allow(clippy::type_complexity)]
pub struct AgentSession {
    agent: Arc<Agent>,
    session_manager: Arc<std::sync::Mutex<SessionManager>>,
    settings_manager: Arc<std::sync::Mutex<crate::core::settings_manager::SettingsManager>>,
    model_registry: ModelRegistry,
    compaction_settings: CompactionSettings,
    cwd: String,
    scoped_models: Vec<(Model, Option<ThinkingLevel>)>,
    initial_active_tool_names: Vec<String>,
    allowed_tool_names: Option<Vec<String>>,
    excluded_tool_names: Option<Vec<String>>,
    /// Extension registry (Rust native extensions).
    extension_registry: Option<Arc<ExtensionRegistry>>,
    /// Cached extension context for dispatch calls.
    ext_ctx: ExtensionContext,
    /// Full tool registry (all available tools, not just active ones),
    /// matching TS `_toolRegistry`. Used by `set_active_tools_by_name()`.
    tool_registry: Vec<Arc<pi_agent_core::types::DynTool>>,
    /// Tool definitions registry, matching TS `_toolDefinitions` (used by
    /// getAllTools / getToolDefinition). Populated from custom_tools and
    /// extension tools at construction time.
    tool_definitions: std::collections::HashMap<String, crate::core::extensions::ToolDefinition>,
    /// Loaded resources (skills, prompt templates, context files), matching TS `_resourceLoader`.
    resources: Option<LoadedResources>,
    /// System-prompt build options (without the selected-tool list, which
    /// changes with the active tool set). Saved at construction so
    /// `set_active_tools_by_name` can rebuild the prompt (matching TS
    /// `_rebuildSystemPrompt`).
    system_prompt_options: Option<system_prompt::BuildSystemPromptOptions>,
    /// The base system prompt (before per-turn extension modifications),
    /// matching TS `_baseSystemPrompt`. Rebuilt when the active tool set
    /// changes; `before_agent_start` dispatches against it so extension
    /// modifications don't accumulate across turns (matching TS which passes
    /// `this._baseSystemPrompt`).
    base_system_prompt: Arc<std::sync::Mutex<String>>,
    /// Extension-contributed resource paths from `resources_discover` event.
    /// These are collected at session start and can be applied to the resource loader
    /// on session reload via `extend_resources()`.
    extension_resource_paths: Option<crate::core::resource_loader::ResourceExtensionPaths>,
    /// Pending bash execution results queued while agent is streaming,
    /// matching TS `_pendingBashMessages`.
    pending_bash_messages: std::sync::Mutex<Vec<serde_json::Value>>,
    /// Shared state snapshot for JS extension read-actions, refreshed at
    /// drain points (turn boundaries).
    extension_state_view: Option<Arc<std::sync::Mutex<crate::core::extensions::action_bus::ExtensionStateView>>>,
    /// Receiver for JS extension write-actions, drained at turn boundaries.
    extension_action_rx: Option<Arc<std::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<crate::core::extensions::action_bus::ExtensionAction>>>>,
    /// Extension error listener, matching TS `bindExtensions({ onError })`.
    /// Set via `bind_extensions()`; invoked when an extension raises an error.
    extension_error_listener: Arc<std::sync::Mutex<Option<ErrorListener>>>,
    /// Sync callback that invalidates the JS extension runtime (stale-ctx
    /// guard) when the session changes (new/fork/switch/reload). Set by the
    /// SDK; absent when no JS extension runtime is loaded.
    js_invalidator: Option<Arc<dyn Fn() + Send + Sync>>,
    // ── Event subscription state ──
    event_listeners: Arc<std::sync::Mutex<Vec<AgentSessionEventListener>>>,
    /// Handle to the internal agent event subscription.
    _agent_subscription: Option<pi_agent_core::agent::UnsubscribeHandle>,
    /// Whether the agent is currently processing a run.
    is_agent_run_active: Arc<std::sync::Mutex<bool>>,
    /// Notifier for idle detection (wakes wait_for_idle callers).
    idle_notify: Arc<Notify>,
    /// Tracks pending steering messages for UI display. Removed when delivered.
    steering_messages: Arc<std::sync::Mutex<Vec<String>>>,
    /// Tracks pending follow-up messages for UI display. Removed when delivered.
    follow_up_messages: Arc<std::sync::Mutex<Vec<String>>>,
    /// Pending next-turn messages (custom messages queued for next prompt()),
    /// matching TS `_pendingNextTurnMessages`.
    pending_next_turn_messages: Arc<std::sync::Mutex<Vec<AgentMessage>>>,
    /// Current retry attempt (0 if not retrying).
    retry_attempt: Arc<std::sync::Mutex<u32>>,
    /// Abort controller for retry backoff, matching TS `_retryAbortController`.
    retry_abort: Arc<std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    /// Whether overflow recovery has been attempted in the current turn.
    overflow_recovery_attempted: Arc<std::sync::Mutex<bool>>,
    /// Last assistant message received, for auto-compaction and retry checks.
    last_assistant_message: Arc<std::sync::Mutex<Option<AgentMessage>>>,
    /// Abort controller for manual compaction.
    compaction_abort: Arc<std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    /// Abort controller for auto-compaction.
    auto_compaction_abort: Arc<std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    /// Abort controller for branch summarization.
    branch_summary_abort: Arc<std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    /// Abort controller for bash execution, matching TS `_bashAbortController`.
    bash_abort: Arc<std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
}

impl AgentSession {
    pub async fn new(
        session_manager: SessionManager,
        settings_manager: crate::core::settings_manager::SettingsManager,
        model_registry: ModelRegistry,
        options: AgentSessionConfig,
    ) -> Self {
        // ── Extension context (needed early for extension tool dispatch) ──
        // RuntimeHandle 提供真实 cwd/agent_dir（noop() 返回空串，扩展 spawn
        // 子进程或写状态文件时会失败/落错位置）。
        let mut ext_runtime_handle = crate::core::extensions::RuntimeHandle::noop();
        let ext_cwd = options.cwd.clone();
        ext_runtime_handle.get_cwd = std::sync::Arc::new(move || ext_cwd.clone());
        let ext_agent_dir = crate::config::get_agent_dir().to_string_lossy().to_string();
        ext_runtime_handle.get_agent_dir = std::sync::Arc::new(move || ext_agent_dir.clone());
        // 当前模型（provider/model），供扩展（如 subagent）继承给子进程。
        let ext_model = format!("{}/{}", options.model.provider, options.model.id);
        ext_runtime_handle.get_model = std::sync::Arc::new(move || ext_model.clone());
        // Extensions can register providers (match TS `registerProvider`, #019e4ad68).
        let registry_for_provider = model_registry.clone();
        ext_runtime_handle.register_provider = std::sync::Arc::new(move |config_value| {
            if let Ok(config) = serde_json::from_value::<crate::core::model_registry::ProviderConfig>(
                config_value,
            ) {
                if let Some(name) = config.name.clone() {
                    registry_for_provider.register_provider(&name, config);
                }
            }
        });
        let ext_ctx = ExtensionContext::new(
            options.cwd.clone(),
            false,
            crate::core::extensions::ExtensionUIContext {
                notify: std::sync::Arc::new(|msg, _level| eprintln!("[pi] {msg}")),
                ..crate::core::extensions::ExtensionUIContext::noop()
            },
            ext_runtime_handle,
        );
        let shared_ext_ctx = Arc::new(ext_ctx);

        // ── Build tool list ──
        let tools_options = tools::ToolsOptions::default();
        let mut tool_list: Vec<pi_agent_core::types::DynTool> = Vec::new();

        // 1. Built-in tools (read, bash, edit, write)
        // Bash tools receive current session metadata as PI_* env vars, resolved
        // at each command start (match TS #6967). The agent reference is bound
        // late (after Agent::new) so model/thinking-level changes are picked up.
        let bash_agent_ref: Arc<std::sync::Mutex<Option<Arc<Agent>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let bash_session_manager = Arc::new(std::sync::Mutex::new(session_manager));
        let bash_session_env_provider: tools::bash::BashSessionEnvProvider = {
            let agent_ref = bash_agent_ref.clone();
            let sm = bash_session_manager.clone();
            Arc::new(move || {
                let agent_ref = agent_ref.clone();
                let sm = sm.clone();
                Box::pin(async move {
                    // Read session metadata in a scoped block so the MutexGuard
                    // is dropped before any await (keeps the future Send).
                    let (session_id, session_file) = {
                        let sm_guard =
                            sm.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        (
                            sm_guard.get_session_id().to_string(),
                            sm_guard
                                .get_session_file()
                                .map(|p| p.to_string_lossy().to_string()),
                        )
                    };
                    // Clone the agent Arc and drop the guard before awaiting.
                    let agent = agent_ref
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    match agent {
                        Some(agent) => {
                            let state = agent.state().await;
                            tools::bash::BashSessionEnv {
                                session_id: Some(session_id),
                                session_file,
                                provider: Some(state.model.provider.clone()),
                                model: Some(state.model.id.clone()),
                                thinking_level: Some(state.thinking_level.clone()),
                            }
                        }
                        None => tools::bash::BashSessionEnv {
                            session_id: Some(session_id),
                            session_file,
                            provider: None,
                            model: None,
                            thinking_level: None,
                        },
                    }
                })
            })
        };
        tool_list.extend(tools::create_coding_tools(
            &options.cwd,
            Some(&tools_options),
            Some(bash_session_env_provider),
        ));

        // 2. Custom tools from SDK callers (via custom_tools / ToolDefinition + execute)
        if let Some(ref custom_tools) = options.custom_tools {
            use pi_agent_core::pi_ai_types::ToolExecutionMode;
            use pi_agent_core::types::AgentToolResult;
            for def in custom_tools {
                let tool_name = def.name.clone();
                let execute: ToolExecuteFn = if let Some(ref tool_exec) = def.execute {
                    let exec = tool_exec.clone();
                    Arc::new(move |id, params, signal, _on_update| {
                        let exec = exec.clone();
                        Box::pin(async move {
                            let output = exec(id.clone(), params, signal).await?;
                            let content: Vec<pi_agent_core::pi_ai_types::ContentBlock> = output
                                .content
                                .into_iter()
                                .filter_map(|v| serde_json::from_value(v).ok())
                                .collect();
                            Ok(AgentToolResult {
                                content,
                                details: output.details.unwrap_or(serde_json::Value::Null),
                                usage: None,
                                added_tool_names: None,
                                terminate: output.terminate,
                            })
                        })
                    })
                } else {
                    Arc::new(move |_id, _params, _signal, _callback| {
                        let err: Box<dyn std::error::Error + Send + Sync> = format!(
                            "Tool '{tool_name}' has no execute — call agent.add_tools() to provide one"
                        ).into();
                        Box::pin(async move { Err(err) })
                    })
                };
                tool_list.push(pi_agent_core::types::AgentTool {
                    name: def.name.clone(),
                    description: def.description.clone(),
                    label: def.label.clone().unwrap_or_default(),
                    prompt_snippet: def.prompt_snippet.clone(),
                    prompt_guidelines: def.prompt_guidelines.clone(),
                    parameters_schema: def.parameters.clone().unwrap_or(
                        serde_json::json!({"type": "object", "properties": {}, "required": []}),
                    ),
                    execution_mode: def.execution_mode.as_deref().and_then(|m| match m {
                        "sequential" => Some(ToolExecutionMode::Sequential),
                        "parallel" => Some(ToolExecutionMode::Parallel),
                        _ => None,
                    }),
                    prepare_arguments: None,
                    execute,
                });
            }
        }

        // 3. Extension tools from ExtensionRegistry (via handle_tool_call dispatch)
        //    Matches TS _refreshToolRegistry wrapping extension tools into AgentTool entries.
        if let Some(ref registry) = options.extension_registry {
            use pi_agent_core::pi_ai_types::ToolExecutionMode;
            use pi_agent_core::types::AgentToolResult;
            let collected = registry.tools().to_vec();
            for rt in collected {
                let def = rt.definition;
                let ext_tool_name = def.name.clone();
                let ext_reg = Arc::clone(registry);
                let ext_ctx_clone = Arc::clone(&shared_ext_ctx);
                let execute: ToolExecuteFn = Arc::new(move |_id, params, _signal, _on_update| {
                    let reg = Arc::clone(&ext_reg);
                    let ctx = Arc::clone(&ext_ctx_clone);
                    let name = ext_tool_name.clone();
                    Box::pin(async move {
                        match crate::core::extensions::dispatcher::dispatch_handle_tool_call(
                            &reg, &name, params, &ctx,
                        )
                        .await
                        {
                            Some(output) => {
                                let content: Vec<pi_agent_core::pi_ai_types::ContentBlock> = output
                                    .content
                                    .into_iter()
                                    .filter_map(|v| serde_json::from_value(v).ok())
                                    .collect();
                                Ok(AgentToolResult {
                                    content,
                                    details: output.details.unwrap_or(serde_json::Value::Null),
                                    usage: None,
                                    added_tool_names: None,
                                    terminate: output.terminate,
                                })
                            }
                            None => {
                                Err(format!("Tool '{name}' not handled by any extension").into())
                            }
                        }
                    })
                });
                tool_list.push(pi_agent_core::types::AgentTool {
                    name: def.name,
                    description: def.description,
                    label: def.label.unwrap_or_default(),
                    prompt_snippet: def.prompt_snippet,
                    prompt_guidelines: def.prompt_guidelines,
                    parameters_schema: def.parameters.unwrap_or(
                        serde_json::json!({"type": "object", "properties": {}, "required": []}),
                    ),
                    execution_mode: def.execution_mode.as_deref().and_then(|m| match m {
                        "sequential" => Some(ToolExecutionMode::Sequential),
                        "parallel" => Some(ToolExecutionMode::Parallel),
                        _ => None,
                    }),
                    prepare_arguments: None,
                    execute,
                });
            }
        }
        // Save full tool list as registry (before filtering/activation).
        let tool_registry: Vec<Arc<pi_agent_core::types::DynTool>> = tool_list
            .iter()
            .map(|t| Arc::new(t.clone()) as Arc<pi_agent_core::types::DynTool>)
            .collect();

        // 4. Filter tool list by allowed/excluded names (matching TS isAllowedTool)
        if let Some(ref allowed) = options.allowed_tool_names {
            tool_list.retain(|t| allowed.contains(&t.name));
        }
        if let Some(ref excluded) = options.excluded_tool_names {
            tool_list.retain(|t| !excluded.contains(&t.name));
        }

        // 5. Build system prompt with tool metadata from ALL active tools.
        //    Matches TS _rebuildSystemPrompt(validToolNames) which runs after
        //    tool registry refresh and includes tool snippets/guidelines.
        //    Built-in tools carry their own prompt_snippet / prompt_guidelines
        //    on the AgentTool (matching TS ToolDefinition.promptSnippet).
        let tool_snippets: std::collections::HashMap<String, String> = {
            let mut map = options.tool_snippets.clone().unwrap_or_default();
            for tool in &tool_list {
                if let Some(ref snippet) = tool.prompt_snippet {
                    // Collapse all whitespace runs to single spaces (matching
                    // TS _normalizePromptSnippet: \r\n → space, \s+ → space).
                    let normalized = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !normalized.is_empty() {
                        map.insert(tool.name.clone(), normalized);
                    }
                }
            }
            if let Some(ref custom_tools) = options.custom_tools {
                for def in custom_tools {
                    if let Some(ref snippet) = def.prompt_snippet {
                        let normalized =
                            snippet.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !normalized.is_empty() {
                            map.insert(def.name.clone(), normalized);
                        }
                    }
                }
            }
            map
        };

        let prompt_guidelines: Vec<String> = {
            let mut guidelines = options.prompt_guidelines.clone().unwrap_or_default();
            for tool in &tool_list {
                if let Some(ref g) = tool.prompt_guidelines {
                    for line in g {
                        let trimmed = line.trim().to_string();
                        if !trimmed.is_empty() {
                            guidelines.push(trimmed);
                        }
                    }
                }
            }
            if let Some(ref custom_tools) = options.custom_tools {
                for def in custom_tools {
                    if let Some(ref g) = def.prompt_guidelines {
                        for line in g {
                            let trimmed = line.trim().to_string();
                            if !trimmed.is_empty() {
                                guidelines.push(trimmed);
                            }
                        }
                    }
                }
            }
            guidelines
        };

        let selected_tool_names: Vec<String> = tool_list.iter().map(|t| t.name.clone()).collect();
        let system_prompt_options = system_prompt::BuildSystemPromptOptions {
            cwd: options.cwd.clone(),
            custom_prompt: options.custom_prompt,
            append_system_prompt: options.append_system_prompt,
            selected_tools: Some(selected_tool_names),
            tool_snippets: Some(tool_snippets),
            prompt_guidelines: Some(prompt_guidelines),
            context_files: Some(options.context_files),
            skills: Some(options.skills),
        };
        let system_prompt = system_prompt::build_system_prompt(&system_prompt_options);

        // 6. Apply initial_active_tool_names: only built-in tools are gated
        //    by this; custom + extension tools are always active (matching
        //    TS includeAllExtensionTools: true).
        let initial_active = options
            .initial_active_tool_names
            .clone()
            .unwrap_or_default();
        let custom_names: std::collections::HashSet<String> = options
            .custom_tools
            .as_ref()
            .map(|ct| ct.iter().map(|d| d.name.clone()).collect())
            .unwrap_or_default();
        // Built-in tool names (read/bash/edit/write) are the only ones subject
        // to gating by initial_active_tool_names. Extension tools (e.g. goal)
        // and custom tools are always active.
        let builtin_names: std::collections::HashSet<String> =
            tools::create_coding_tools(&options.cwd, None, None)
                .iter()
                .map(|t| t.name.clone())
                .collect();
        tool_list.retain(|t| {
            custom_names.contains(&t.name)
                || !builtin_names.contains(&t.name)
                || initial_active.contains(&t.name)
        });
        let tools: Vec<Arc<pi_agent_core::types::DynTool>> = tool_list
            .into_iter()
            .map(|t| Arc::new(t) as Arc<pi_agent_core::types::DynTool>)
            .collect();

        let initial_state = AgentState {
            system_prompt: system_prompt.clone(),
            model: options.model.clone(),
            thinking_level: options.thinking_level,
            tools,
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        };

        let convert_to_llm = options
            .convert_to_llm
            .unwrap_or_else(|| Arc::new(messages::convert_to_llm));

        let stream_fn = options.stream_fn.unwrap_or_else(|| {
            Arc::new(|_model, _ctx, _thinking, _opts| {
                Box::pin(async {
                    Err::<pi_agent_core::pi_ai_types::StreamResponse, _>(
                        "No stream function configured".into(),
                    )
                })
            })
        });

        // Wire extension before/after_tool_call hooks into the agent's tool
        // execution loop. When an extension registry is present, each tool call
        // is dispatched to extension handlers that may block it (before) or
        // transform its result (after).

        let (before_tool_call, after_tool_call) = match &options.extension_registry {
            Some(registry) => {
                let before_reg = Arc::clone(registry);
                let after_reg = Arc::clone(registry);
                let before_ctx = Arc::clone(&shared_ext_ctx);
                let after_ctx = Arc::clone(&shared_ext_ctx);
                let before: BeforeToolCallFn = Arc::new(move |ctx, _signal| {
                    let reg = Arc::clone(&before_reg);
                    let ctx_ref = Arc::clone(&before_ctx);
                    Box::pin(async move {
                        crate::core::extensions::dispatcher::dispatch_tool_call(
                            &reg, &ctx, &ctx_ref,
                        )
                        .await
                    })
                });
                let after: AfterToolCallFn = Arc::new(move |ctx, _signal| {
                    let reg = Arc::clone(&after_reg);
                    let ctx_ref = Arc::clone(&after_ctx);
                    Box::pin(async move {
                        crate::core::extensions::dispatcher::dispatch_tool_result(
                            &reg, &ctx, &ctx_ref,
                        )
                        .await
                    })
                });
                (Some(before), Some(after))
            }
            None => (None, None),
        };

        // Wire the context event hook: extensions can modify messages before
        // they are sent to the LLM.
        let transform_context: Option<TransformContextFn> =
            options.extension_registry.as_ref().map(|registry| {
                let dispatch_reg = Arc::clone(registry);
                let ctx_clone = Arc::clone(&shared_ext_ctx);
                let closure =
                    move |messages: Vec<AgentMessage>,
                          _signal: Option<tokio::sync::watch::Receiver<bool>>| {
                        let reg = Arc::clone(&dispatch_reg);
                        let ctx_ref = Arc::clone(&ctx_clone);
                        Box::pin(async move {
                            let serialized: Vec<serde_json::Value> = messages
                                .iter()
                                .map(|m| serde_json::to_value(m).unwrap_or_default())
                                .collect();
                            let modified = crate::core::extensions::dispatcher::dispatch_context(
                                &reg,
                                &serialized,
                                &ctx_ref,
                            )
                            .await;
                            // Deserialize modified messages back, or fall back to originals
                            if modified.len() == messages.len() {
                                let deserialized: Vec<Option<AgentMessage>> = modified
                                    .into_iter()
                                    .map(|v| serde_json::from_value(v).ok())
                                    .collect();
                                if let Some(deserialized) =
                                    deserialized.into_iter().collect::<Option<Vec<_>>>()
                                {
                                    return deserialized;
                                }
                            }
                            messages
                        })
                            as std::pin::Pin<
                                Box<dyn std::future::Future<Output = Vec<AgentMessage>> + Send>,
                            >
                    };
                Arc::new(closure) as TransformContextFn
            });

        // Wire the before_provider_request event: extensions can inspect/modify
        // the provider request payload before it is sent.
        // Wire the before_provider_request event: extensions can inspect/modify
        // the provider request payload before it is sent.
        let on_payload: Option<PayloadHookFn> = options.extension_registry.as_ref().map(|registry| {
            let payload_reg = Arc::clone(registry);
            let ctx_clone = Arc::clone(&shared_ext_ctx);
            let closure = move |payload: serde_json::Value| {
                let reg = Arc::clone(&payload_reg);
                let ctx_ref = Arc::clone(&ctx_clone);
                Box::pin(async move {
                    crate::core::extensions::dispatcher::dispatch_before_provider_request(
                        &reg, payload, &ctx_ref,
                    )
                    .await
                })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send>,
                    >
            };
            Arc::new(closure) as PayloadHookFn
        });

        // Wire the before_provider_headers event: extensions can modify
        // HTTP request headers before they are sent to the provider.
        let on_headers: Option<HeadersHookFn> = options.extension_registry.as_ref().map(|registry| {
            let headers_reg = Arc::clone(registry);
            let ctx_clone = Arc::clone(&shared_ext_ctx);
            let closure = move |headers: std::collections::HashMap<String, String>| {
                let reg = Arc::clone(&headers_reg);
                let ctx_ref = Arc::clone(&ctx_clone);
                Box::pin(async move {
                    crate::core::extensions::dispatcher::dispatch_before_provider_headers(
                        &reg, headers, &ctx_ref,
                    )
                    .await
                })
                    as std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = std::collections::HashMap<String, String>,
                                > + Send,
                        >,
                    >
            };
            Arc::new(closure) as HeadersHookFn
        });

        // Wire the after_provider_response event: extensions can inspect
        // provider HTTP response status and headers.
        let on_provider_response: Option<ProviderResponseHookFn> =
            options.extension_registry.as_ref().map(|registry| {
                let resp_reg = Arc::clone(registry);
                let ctx_clone = Arc::clone(&shared_ext_ctx);
                let closure = move |status: u16, headers: std::collections::HashMap<String, String>| {
                    let reg = Arc::clone(&resp_reg);
                    let ctx_ref = Arc::clone(&ctx_clone);
                    tokio::spawn(async move {
                        crate::core::extensions::dispatcher::dispatch_after_provider_response(
                            &reg, status, headers, &ctx_ref,
                        )
                        .await;
                    });
                };
                Arc::new(closure) as ProviderResponseHookFn
            });

        // Wire the API key resolution callback so the agent loop can
        // look up keys from env vars, registered providers, and models.json config.
        let model_registry_for_key = model_registry.clone();
        let get_api_key: Option<pi_agent_core::types::GetApiKeyFn> =
            Some(std::sync::Arc::new(move |provider: String| {
                let registry = model_registry_for_key.clone();
                Box::pin(async move { registry.get_api_key_for_provider(&provider) })
            }));

        let agent_options = pi_agent_core::agent::AgentOptions {
            initial_state: Some(initial_state),
            convert_to_llm: Some(convert_to_llm),
            stream_fn: Some(stream_fn),
            session_id: Some(
                bash_session_manager
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_session_id()
                    .to_string(),
            ),
            before_tool_call,
            after_tool_call,
            transform_context,
            on_payload,
            on_headers,
            on_provider_response,
            get_api_key,
            // Refresh system prompt / tools / model / thinking level before
            // each turn (matching TS _installAgentNextTurnRefresh which sets
            // agent.prepareNextTurnWithContext). The agent reference is bound
            // late via bash_agent_ref (set after Agent::new).
            prepare_next_turn_with_context: {
                let agent_ref = bash_agent_ref.clone();
                Some(Arc::new(move |turn, _signal| {
                    let agent_ref = agent_ref.clone();
                    Box::pin(async move {
                        let agent = agent_ref
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()?;
                        let state = agent.state().await;
                        Some(pi_agent_core::types::AgentLoopTurnUpdate {
                            context: Some(pi_agent_core::types::AgentContext {
                                system_prompt: state.system_prompt,
                                messages: turn.context.messages,
                                tools: Some(state.tools),
                            }),
                            model: Some(state.model),
                            thinking_level: Some(state.thinking_level),
                        })
                    })
                }))
            },
            ..Default::default()
        };

        let agent = Arc::new(Agent::new(agent_options));
        // Bind the agent reference used by the bash tool's session-env provider
        // (resolved at each command start, match TS #6967).
        *bash_agent_ref
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(agent.clone());
        let session_manager = bash_session_manager;

        let initial_active_tool_names = options.initial_active_tool_names.unwrap_or_else(|| {
            ["read", "bash", "edit", "write"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        });

        let session_cwd = options.cwd.clone();
        let session_cwd_for_ext = session_cwd.clone();
        // Build tool definitions registry (matching TS `_toolDefinitions`).
        let mut tool_definitions: std::collections::HashMap<
            String,
            crate::core::extensions::ToolDefinition,
        > = std::collections::HashMap::new();
        if let Some(ref custom_tools) = options.custom_tools {
            for def in custom_tools {
                let mut def = def.clone();
                // SDK tools carry a synthetic source (matching TS
                // createSyntheticSourceInfo(`<sdk:${name}>`, { source: "sdk" })).
                def.source_info = Some(crate::core::extensions::create_synthetic_source_info(
                    format!("<sdk:{}>", def.name),
                    "sdk".to_string(),
                    None,
                    None,
                    None,
                ));
                tool_definitions.insert(def.name.clone(), def);
            }
        }
        if let Some(ref registry) = options.extension_registry {
            for rt in registry.tools().to_vec() {
                let mut def = rt.definition;
                def.source_info = Some(rt.source_info);
                tool_definitions
                    .entry(def.name.clone())
                    .or_insert(def);
            }
        }

        // Clone registry ref for EventPublisher (before it's moved into self)
        let extension_registry_ref = options
            .extension_registry
            .as_ref()
            .map(std::sync::Arc::clone);

        // Clone registry ref for extension provider registration (before it's moved into self)
        let model_registry_for_ext = model_registry.clone();

        let mut session = Self {
            agent,
            session_manager: session_manager.clone(),
            settings_manager: Arc::new(std::sync::Mutex::new(settings_manager)),
            model_registry,
            compaction_settings: CompactionSettings::default(),
            cwd: session_cwd.clone(),
            scoped_models: Vec::new(),
            initial_active_tool_names,
            allowed_tool_names: options.allowed_tool_names,
            excluded_tool_names: options.excluded_tool_names,
            extension_registry: options.extension_registry,
            ext_ctx: {
                // RuntimeHandle 提供真实 cwd/agent_dir（noop() 返回空串，
                // 扩展 spawn 子进程或写状态文件时会失败/落错位置）。
                let mut ext_runtime_handle = crate::core::extensions::RuntimeHandle::noop();
                let ext_cwd = session_cwd_for_ext.clone();
                ext_runtime_handle.get_cwd = std::sync::Arc::new(move || ext_cwd.clone());
                let ext_agent_dir = crate::config::get_agent_dir().to_string_lossy().to_string();
                ext_runtime_handle.get_agent_dir = std::sync::Arc::new(move || ext_agent_dir.clone());
                // Extensions can register providers (match TS `registerProvider`, #019e4ad68).
                let registry_for_provider = model_registry_for_ext.clone();
                ext_runtime_handle.register_provider = std::sync::Arc::new(move |config_value| {
                    if let Ok(config) =
                        serde_json::from_value::<crate::core::model_registry::ProviderConfig>(
                            config_value,
                        )
                    {
                        if let Some(name) = config.name.clone() {
                            registry_for_provider.register_provider(&name, config);
                        }
                    }
                });
                ExtensionContext::new(
                    session_cwd_for_ext.clone(),
                    false,
                    crate::core::extensions::ExtensionUIContext {
                        notify: std::sync::Arc::new(|msg, _level| eprintln!("[pi] {msg}")),
                        ..crate::core::extensions::ExtensionUIContext::noop()
                    },
                    ext_runtime_handle,
                )
            },
            tool_registry,
            tool_definitions,
            resources: options.resources,
            system_prompt_options: Some({
                // Save the build options without the selected-tool list so
                // set_active_tools_by_name can rebuild the prompt when the
                // active tool set changes (matching TS _rebuildSystemPrompt).
                let mut opts = system_prompt_options.clone();
                opts.selected_tools = None;
                opts
            }),
            base_system_prompt: Arc::new(std::sync::Mutex::new(system_prompt.clone())),
            extension_resource_paths: None,
            pending_bash_messages: std::sync::Mutex::new(Vec::new()),
            extension_state_view: options.extension_state_view,
            extension_action_rx: options
                .extension_action_rx
                .map(|rx| Arc::new(std::sync::Mutex::new(rx))),
            js_invalidator: None,
            extension_error_listener: Arc::new(std::sync::Mutex::new(None)),
            event_listeners: Arc::new(std::sync::Mutex::new(Vec::new())),
            _agent_subscription: None,
            is_agent_run_active: Arc::new(std::sync::Mutex::new(false)),
            idle_notify: Arc::new(Notify::new()),
            steering_messages: Arc::new(std::sync::Mutex::new(Vec::new())),
            follow_up_messages: Arc::new(std::sync::Mutex::new(Vec::new())),
            pending_next_turn_messages: Arc::new(std::sync::Mutex::new(Vec::new())),
            retry_attempt: Arc::new(std::sync::Mutex::new(0)),
            retry_abort: Arc::new(std::sync::Mutex::new(None)),
            overflow_recovery_attempted: Arc::new(std::sync::Mutex::new(false)),
            last_assistant_message: Arc::new(std::sync::Mutex::new(None)),
            compaction_abort: Arc::new(std::sync::Mutex::new(None)),
            auto_compaction_abort: Arc::new(std::sync::Mutex::new(None)),
            branch_summary_abort: Arc::new(std::sync::Mutex::new(None)),
            bash_abort: Arc::new(std::sync::Mutex::new(None)),
        };

        // ── Dispatch resources_discover event to extensions ──
        // Notifies extensions that resources have been loaded, allowing them
        // to contribute additional resource paths (skillPaths, promptPaths, themePaths).
        // The returned paths are stored for future use (e.g., when reloading resources).
        if let Some(ref registry) = session.extension_registry {
            let ext_ctx = session.ext_ctx.clone();
            let cwd = session.cwd.clone();
            let ext_paths = crate::core::extensions::dispatcher::dispatch_resources_discover(
                registry,
                &cwd,
                "session_start",
                &ext_ctx,
            )
            .await;
            // Store extension-contributed paths for future resource reloads
            if !ext_paths.skill_paths.is_empty()
                || !ext_paths.prompt_paths.is_empty()
                || !ext_paths.theme_paths.is_empty()
            {
                // Convert ResourcesDiscoverResult (Vec<String>) to ResourceExtensionPaths (Vec<(String, SourceInfo)>)
                use crate::core::source_info::{SourceInfo, SourceOrigin, SourceScope};
                let ext_resource_paths = crate::core::resource_loader::ResourceExtensionPaths {
                    skill_paths: ext_paths
                        .skill_paths
                        .iter()
                        .map(|p| {
                            (
                                p.clone(),
                                SourceInfo {
                                    path: p.clone(),
                                    source: "extension".to_string(),
                                    scope: SourceScope::Project,
                                    origin: SourceOrigin::Package,
                                    base_dir: None,
                                },
                            )
                        })
                        .collect(),
                    prompt_paths: ext_paths
                        .prompt_paths
                        .iter()
                        .map(|p| {
                            (
                                p.clone(),
                                SourceInfo {
                                    path: p.clone(),
                                    source: "extension".to_string(),
                                    scope: SourceScope::Project,
                                    origin: SourceOrigin::Package,
                                    base_dir: None,
                                },
                            )
                        })
                        .collect(),
                    theme_paths: ext_paths
                        .theme_paths
                        .iter()
                        .map(|p| {
                            (
                                p.clone(),
                                SourceInfo {
                                    path: p.clone(),
                                    source: "extension".to_string(),
                                    scope: SourceScope::Project,
                                    origin: SourceOrigin::Package,
                                    base_dir: None,
                                },
                            )
                        })
                        .collect(),
                };
                session.extension_resource_paths = Some(ext_resource_paths);
                eprintln!(
                    "[pi] Extension-contributed paths: {} skills, {} prompts, {} themes",
                    ext_paths.skill_paths.len(),
                    ext_paths.prompt_paths.len(),
                    ext_paths.theme_paths.len(),
                );
            }
        }

        // ── Register internal agent event handler ──
        // Folds persistence, extension dispatch, and session event dispatch
        // into a single subscription, matching TS `_handleAgentEvent`.
        let inner_sm = session_manager.clone();
        let inner_reg = extension_registry_ref.clone();
        let inner_cwd = session_cwd.clone();
        let inner_listeners = session.event_listeners.clone();
        let inner_steering = session.steering_messages.clone();
        let inner_follow_up = session.follow_up_messages.clone();
        let inner_last_assistant = session.last_assistant_message.clone();
        let _inner_retry = session.retry_attempt.clone();
        let _inner_overflow = session.overflow_recovery_attempted.clone();
        let _inner_is_active = session.is_agent_run_active.clone();
        let _inner_idle = session.idle_notify.clone();
        let inner_ext_ctx = shared_ext_ctx.clone();
        let inner_agent = session.agent.clone();
        let inner_settings = session.settings_manager.clone();
        let turn_index: Arc<std::sync::Mutex<u32>> = Arc::new(std::sync::Mutex::new(0));

        let internal_listener: AgentEventListener = Arc::new(move |event: AgentEvent, _signal| {
            let sm = inner_sm.clone();
            let reg = inner_reg.clone();
            let ext_ctx = inner_ext_ctx.clone();
            let _cwd = inner_cwd.clone();
            let listeners = inner_listeners.clone();
            let steering = inner_steering.clone();
            let follow_up = inner_follow_up.clone();
            let last_assistant = inner_last_assistant.clone();
            let _retry = _inner_retry.clone();
            let _overflow = _inner_overflow.clone();
            let _is_active = _inner_is_active.clone();
            let _idle = _inner_idle.clone();
            let agent = inner_agent.clone();
            let turn_index = turn_index.clone();
            let inner_settings = inner_settings.clone();

            Box::pin(async move {
                // ── 1. Handle queue updates and state resets ──
                // Reset overflow recovery on any new user message (matching TS)
                if let AgentEvent::MessageStart { ref message } = event {
                    if let AgentMessage::User { .. } = message {
                        *_overflow.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = false;
                    }
                    if let AgentMessage::User { ref content, .. } = message {
                        let message_text: String = content
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::Text { text, .. } = b {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<&str>>()
                            .join("");
                        if !message_text.is_empty() {
                            // Check steering queue first
                            let (steer_idx, evt) = {
                                let mut steer = steering.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                let idx = steer.iter().position(|m| m == &message_text);
                                if let Some(idx) = idx {
                                    steer.remove(idx);
                                    let follow = follow_up.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                    let evt = AgentSessionEvent::QueueUpdate {
                                        steering: steer.clone(),
                                        follow_up: follow.clone(),
                                    };
                                    (Some(idx), evt)
                                } else {
                                    (
                                        None,
                                        AgentSessionEvent::QueueUpdate {
                                            steering: steer.clone(),
                                            follow_up: follow_up.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(),
                                        },
                                    )
                                }
                            };
                            if steer_idx.is_some() {
                                let batch: Vec<_> = {
                                    let l = listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                    l.iter().cloned().collect()
                                };
                                for listener in batch {
                                    listener(evt.clone());
                                }
                            } else {
                                // Check follow-up queue
                                let evt = {
                                    let mut follow = follow_up.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                    let follow_idx = follow.iter().position(|m| m == &message_text);
                                    if let Some(idx) = follow_idx {
                                        follow.remove(idx);
                                        let steer = steering.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                        Some(AgentSessionEvent::QueueUpdate {
                                            steering: steer.clone(),
                                            follow_up: follow.clone(),
                                        })
                                    } else {
                                        None
                                    }
                                };
                                if let Some(evt) = evt {
                                    let batch: Vec<_> = {
                                        let l = listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                        l.iter().cloned().collect()
                                    };
                                    for listener in batch {
                                        listener(evt.clone());
                                    }
                                }
                            }
                        }
                    }
                }

                // ── 2. Emit to extensions via HookRunner ──
                if let Some(ref registry) = reg {
                    let hr = registry.hook_runner();
                    match &event {
                        AgentEvent::AgentStart => {
                            *turn_index.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
                            hr.fire_agent_start().await;
                        }
                        AgentEvent::AgentEnd { messages } => {
                            let msgs: Vec<serde_json::Value> = messages
                                .iter()
                                .map(|m| serde_json::to_value(m).unwrap_or_default())
                                .collect();
                            hr.fire_agent_end(&msgs).await;
                        }
                        AgentEvent::TurnStart => {
                            let ti = *turn_index.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            hr.fire_turn_start(ti).await;
                        }
                        AgentEvent::TurnEnd {
                            message,
                            tool_results,
                        } => {
                            let ti = *turn_index.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            let msg_val = serde_json::to_value(message).unwrap_or_default();
                            let tr_val: Vec<serde_json::Value> = tool_results
                                .iter()
                                .map(|tr| serde_json::to_value(tr).unwrap_or_default())
                                .collect();
                            hr.fire_turn_end(ti, &msg_val, &tr_val).await;
                            *turn_index.lock().unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
                        }
                        AgentEvent::MessageStart { message } => {
                            let msg_val = serde_json::to_value(message).unwrap_or_default();
                            hr.fire_message_start(&msg_val).await;
                        }
                        AgentEvent::MessageUpdate { message, .. } => {
                            let msg_val = serde_json::to_value(message).unwrap_or_default();
                            hr.fire_message_update(&msg_val).await;
                        }
                        AgentEvent::MessageEnd { message } => {
                            let msg_val = serde_json::to_value(message).unwrap_or_default();
                            // Fire void hook (notification) to all handlers
                            hr.fire_message_end(&msg_val).await;
                            // Run modifying hook to allow extensions to modify the message
                            if let Some(ref registry) = reg {
                                if let Some(mut replacement_val) =
                                    crate::core::extensions::dispatcher::dispatch_message_end(
                                        registry, &msg_val, &ext_ctx,
                                    )
                                    .await
                                {
                                    // Normalize null content to empty array, matching TS behavior:
                                    // extension handlers can return messages with null/missing content;
                                    // normalize so it never enters agent state or session history.
                                    if let Some(role) =
                                        replacement_val.get("role").and_then(|r| r.as_str())
                                    {
                                        let needs_normalize = matches!(
                                            role,
                                            "user" | "assistant" | "toolResult" | "custom"
                                        ) && replacement_val
                                            .get("content")
                                            .map(|c| c.is_null())
                                            .unwrap_or(false);
                                        if needs_normalize {
                                            if let Some(obj) = replacement_val.as_object_mut() {
                                                obj.insert(
                                                    "content".to_string(),
                                                    serde_json::Value::Array(Vec::new()),
                                                );
                                            }
                                        }
                                    }
                                    if let Ok(replacement_msg) = serde_json::from_value::<
                                        pi_agent_core::types::AgentMessage,
                                    >(
                                        replacement_val
                                    ) {
                                        agent.replace_last_message(replacement_msg).await;
                                    }
                                }
                            }
                        }
                        AgentEvent::ToolExecutionStart {
                            tool_call_id,
                            tool_name,
                            args,
                        } => {
                            hr.fire_tool_execution_start(
                                tool_call_id.as_str(),
                                tool_name.as_str(),
                                args,
                            )
                            .await;
                        }
                        AgentEvent::ToolExecutionUpdate {
                            tool_call_id,
                            tool_name,
                            args,
                            partial_result,
                        } => {
                            hr.fire_tool_execution_update(
                                tool_call_id.as_str(),
                                tool_name.as_str(),
                                args,
                                partial_result,
                            )
                            .await;
                        }

                        AgentEvent::ToolExecutionEnd {
                            tool_call_id,
                            tool_name,
                            result,
                            is_error,
                        } => {
                            hr.fire_tool_execution_end(
                                tool_call_id.as_str(),
                                tool_name.as_str(),
                                result,
                                *is_error,
                            )
                            .await;
                        }
                    }
                }
                // ── 3. Emit to session event listeners ──
                let session_event = match &event {
                    AgentEvent::AgentEnd { messages } => {
                        // Compute will_retry, matching TS _handleAgentEvent
                        // which calls _willRetryAfterAgentEnd(event).
                        let retry_settings = inner_settings
                            .clone()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .get_retry_settings();
                        let retry_attempt =
                            *_retry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        // TS passes this.model?.contextWindow ?? 0; the
                        // listener cannot await agent state, so usage-based
                        // overflow cases are disabled (error patterns only).
                        let will_retry = will_retry_after_agent_end(
                            messages,
                            retry_attempt,
                            &retry_settings,
                            0,
                        );
                        AgentSessionEvent::AgentEnd {
                            messages: messages.clone(),
                            will_retry,
                        }
                    }
                    AgentEvent::AgentStart => AgentSessionEvent::AgentStart,
                    AgentEvent::TurnStart => AgentSessionEvent::TurnStart,
                    AgentEvent::TurnEnd {
                        message,
                        tool_results,
                    } => AgentSessionEvent::TurnEnd {
                        message: message.clone(),
                        tool_results: tool_results.clone(),
                    },
                    AgentEvent::MessageStart { message } => AgentSessionEvent::MessageStart {
                        message: message.clone(),
                    },
                    AgentEvent::MessageUpdate {
                        message,
                        assistant_message_event,
                    } => AgentSessionEvent::MessageUpdate {
                        message: message.clone(),
                        assistant_message_event: assistant_message_event.clone(),
                    },
                    AgentEvent::MessageEnd { message } => AgentSessionEvent::MessageEnd {
                        message: message.clone(),
                    },
                    AgentEvent::ToolExecutionStart {
                        tool_call_id,
                        tool_name,
                        args,
                    } => AgentSessionEvent::ToolExecutionStart {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                        args: args.clone(),
                    },
                    AgentEvent::ToolExecutionUpdate {
                        tool_call_id,
                        tool_name,
                        args,
                        partial_result,
                    } => AgentSessionEvent::ToolExecutionUpdate {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                        args: args.clone(),
                        partial_result: partial_result.clone(),
                    },
                    AgentEvent::ToolExecutionEnd {
                        tool_call_id,
                        tool_name,
                        result,
                        is_error,
                    } => AgentSessionEvent::ToolExecutionEnd {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                        result: result.clone(),
                        is_error: *is_error,
                    },
                };
                {
                    let batch: Vec<_> = {
                        let l = listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        l.iter().cloned().collect::<Vec<_>>()
                    };
                    for listener in batch {
                        listener(session_event.clone());
                    }
                }

                // ── 4. Handle session persistence ──
                // NOTE: We re-read the last message from agent state rather than
                // using event.message directly. This matches TS _replaceMessageInPlace
                // semantics: extensions may modify the message in step 2 via
                // agent.replace_last_message(), and persistence must use the
                // replacement, not the original event reference.
                if let AgentEvent::MessageEnd { .. } = event {
                    let state = agent.state().await;
                    let persist_msg = state.messages.last().cloned();
                    if let Some(ref message) = persist_msg {
                        match message {
                            AgentMessage::Custom {
                                custom_type,
                                content,
                                display,
                                details,
                                ..
                            } => {
                                let content_json = serde_json::to_value(content)
                                    .unwrap_or(serde_json::Value::Null);
                                if let Ok(mut mgr) = sm.lock() {
                                    mgr.append_custom_message_entry(
                                        custom_type,
                                        content_json,
                                        *display,
                                        details.clone(),
                                    );
                                }
                            }
                            AgentMessage::User { .. }
                            | AgentMessage::Assistant { .. }
                            | AgentMessage::ToolResult { .. } => {
                                let msg_value = serde_json::to_value(message)
                                    .unwrap_or(serde_json::Value::Null);
                                if let Ok(mut mgr) = sm.lock() {
                                    mgr.append_message(msg_value);
                                }
                            }
                            _ => {}
                        }

                        // Track assistant message for auto-compaction
                        if let AgentMessage::Assistant { stop_reason, .. } = message {
                            *last_assistant.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message.clone());
                            // Reset overflow recovery and emit auto_retry_end on successful
                            // assistant response (matching TS _handleAgentEvent)
                            if stop_reason != &Some(pi_agent_core::pi_ai_types::StopReason::Error) {
                                *_overflow.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = false;
                                let retry = *_retry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                if retry > 0 {
                                    let evt = AgentSessionEvent::AutoRetryEnd {
                                        success: true,
                                        attempt: retry,
                                        final_error: None,
                                    };
                                    let batch: Vec<_> = {
                                        let l = listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                        l.iter().cloned().collect()
                                    };
                                    for listener in &batch {
                                        listener(evt.clone());
                                    }
                                    *_retry.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
                                }
                            }
                        }
                    }
                }
            })
        });

        let _subscription_handle = session.agent.subscribe(internal_listener).await;
        session._agent_subscription = Some(_subscription_handle);

        session
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    pub fn get_agent(&self) -> &Agent {
        self.agent.as_ref()
    }

    /// Get full agent state, matching TS `get state()`.
    pub async fn get_state(&self) -> AgentState {
        self.agent.state().await
    }

    pub async fn get_messages(&self) -> Vec<AgentMessage> {
        self.agent.state().await.messages
    }

    /// Load messages from the session manager's file entries into
    /// the agent's in-memory state. Called after restoring from a JSONL file.
    pub async fn load_messages_from_session(&self) -> usize {
        use crate::core::session_manager::SessionEntry;

        let agent_messages = {
            let mgr = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if mgr.get_session_file().is_none() {
                return 0;
            }
            mgr.get_entries()
                .iter()
                .filter_map(|entry| {
                    if let SessionEntry::Message { message, .. } = entry {
                        serde_json::from_value(message.clone()).ok()
                    } else {
                        None
                    }
                })
                .collect::<Vec<AgentMessage>>()
        };
        let count = agent_messages.len();
        if count > 0 {
            self.agent.set_initial_messages(agent_messages).await;
        }
        count
    }

    pub async fn get_system_prompt(&self) -> String {
        self.agent.state().await.system_prompt
    }

    pub async fn get_model(&self) -> Model {
        self.agent.state().await.model
    }

    pub async fn get_thinking_level(&self) -> ThinkingLevel {
        self.agent.state().await.thinking_level
    }

    pub fn get_cwd(&self) -> &str {
        &self.cwd
    }

    pub fn get_extension_registry(&self) -> Option<Arc<ExtensionRegistry>> {
        self.extension_registry.clone()
    }

    pub fn get_session_id(&self) -> String {
        self.session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_session_id()
            .to_string()
    }

    pub fn get_session_file(&self) -> Option<std::path::PathBuf> {
        self.session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_session_file()
            .map(|p| p.to_path_buf())
    }

    pub fn get_session_dir(&self) -> std::path::PathBuf {
        self.session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_session_dir()
            .to_path_buf()
    }

    pub fn get_session_name(&self) -> Option<String> {
        self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_session_name()
    }

    pub fn set_session_name(&self, name: &str) {
        self.session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append_session_info(name);
        // Emit session_info_changed to session listeners (matching TS
        // setSessionName which calls this._emit).
        self._emit(AgentSessionEvent::SessionInfoChanged {
            name: Some(name.to_string()),
        });
        // Dispatch session_info_changed to extensions
        if let Some(ref registry) = self.extension_registry {
            let reg = Arc::clone(registry);
            let name = name.to_string();
            // ExtensionContext is not Clone, so create a no-op context for fire-and-forget
            let ext_ctx = crate::core::extensions::ExtensionContext::new(
                String::new(),
                false,
                crate::core::extensions::ExtensionUIContext::noop(),
                crate::core::extensions::RuntimeHandle::noop(),
            );
            tokio::spawn(async move {
                crate::core::extensions::dispatcher::dispatch_session_info_changed(
                    &reg,
                    Some(&name),
                    &ext_ctx,
                )
                .await;
            });
        }
    }

    /// Whether the session is currently processing an agent run or post-run
    /// continuation (retry / compaction / queued), matching TS `get isStreaming()`
    /// which returns `_isAgentRunActive`.
    pub async fn is_streaming(&self) -> bool {
        *self
            .is_agent_run_active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the session has no active agent run, retry, auto-compaction, or
    /// queued continuation, matching TS `get isIdle()`.
    pub async fn is_idle(&self) -> bool {
        !*self
            .is_agent_run_active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub async fn get_error_message(&self) -> Option<String> {
        self.agent.state().await.error_message.clone()
    }

    /// Get context usage information, matching TS getContextUsage().
    /// Returns None if no model is set or context window is unknown.
    pub async fn get_context_usage(&self) -> Option<ContextUsage> {
        let state = self.agent.state().await;
        let context_window = state.model.context_window;
        if context_window == 0 {
            return None;
        }

        // After compaction, the last assistant usage reflects pre-compaction context size.
        // We can only trust usage from an assistant that responded after the latest compaction.
        // If no such assistant exists, context token count is unknown until the next LLM response.
        let mgr = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let branch_entries = mgr.get_branch(None);

        // Find the latest compaction entry
        let latest_compaction_idx = branch_entries
            .iter()
            .rposition(|e| matches!(e, SessionEntry::Compaction { .. }));

        if let Some(compaction_idx) = latest_compaction_idx {
            // Check if there's a valid assistant usage after the compaction boundary
            let mut has_post_compaction_usage = false;
            for i in (compaction_idx + 1..branch_entries.len()).rev() {
                if let SessionEntry::Message { message, .. } = &branch_entries[i] {
                    if message.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                        let stop_reason = message.get("stopReason").and_then(|r| r.as_str());
                        // Skip aborted and error messages
                        if stop_reason != Some("aborted") && stop_reason != Some("error") {
                            if let Some(usage) = message.get("usage") {
                                let total_tokens = usage
                                    .get("totalTokens")
                                    .and_then(|t| t.as_u64())
                                    .unwrap_or(0);
                                if total_tokens > 0 {
                                    has_post_compaction_usage = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if !has_post_compaction_usage {
                return None;
            }
        }
        drop(mgr);

        // Estimate tokens from current messages
        let messages = &state.messages;
        let total_tokens = crate::core::compaction::estimate_agent_messages_tokens(messages);

        Some(ContextUsage {
            total_tokens,
            context_window,
            input_tokens: total_tokens,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_write_tokens: None,
            messages_count: messages.len(),
        })
    }

    /// Check whether compaction should be triggered, matching TS shouldCompact().
    pub async fn should_compact(&self) -> bool {
        self.check_auto_compact().await
    }

    /// Get text content of the last assistant message, matching TS.
    pub async fn get_last_assistant_text(&self) -> Option<String> {
        let messages = self.agent.state().await.messages;
        for msg in messages.iter().rev() {
            if let AgentMessage::Assistant {
                content,
                stop_reason,
                ..
            } = msg
            {
                // Skip aborted messages with no content (matching TS behavior)
                if stop_reason == &Some(pi_agent_core::pi_ai_types::StopReason::Aborted)
                    && content.is_empty()
                {
                    continue;
                }
                let text: String = content
                    .iter()
                    .filter_map(|block| {
                        if let ContentBlock::Text { text, .. } = block {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<&str>>()
                    .join("")
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }

    pub fn get_scoped_models(&self) -> &[(Model, Option<ThinkingLevel>)] {
        &self.scoped_models
    }

    pub fn set_scoped_models(&mut self, models: Vec<(Model, Option<ThinkingLevel>)>) {
        self.scoped_models = models;
    }

    pub fn get_session_manager(&self) -> std::sync::MutexGuard<'_, SessionManager> {
        self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn get_model_registry(&self) -> &ModelRegistry {
        &self.model_registry
    }

    pub fn get_compaction_settings(&self) -> &CompactionSettings {
        &self.compaction_settings
    }

    pub fn set_compaction_settings(&mut self, settings: CompactionSettings) {
        self.compaction_settings = settings;
    }

    pub fn get_initial_active_tool_names(&self) -> &[String] {
        &self.initial_active_tool_names
    }

    pub fn get_allowed_tool_names(&self) -> Option<&[String]> {
        self.allowed_tool_names.as_deref()
    }

    pub fn get_excluded_tool_names(&self) -> Option<&[String]> {
        self.excluded_tool_names.as_deref()
    }

    /// Get pending steering messages (read-only), matching TS getSteeringMessages().
    /// Check if any extension has registered handlers for the given event type,
    /// matching TS `hasExtensionHandlers(eventType)`.
    pub fn has_extension_handlers(&self, _event_type: &str) -> bool {
        self.extension_registry
            .as_ref()
            .map(|r| r.has_handlers())
            .unwrap_or(false)
    }

    /// Get the extension registry (equivalent to TS `get extensionRunner()`).
    /// Panics if no extension registry is configured.
    pub fn extension_runner(&self) -> &Arc<ExtensionRegistry> {
        self.extension_registry
            .as_ref()
            .unwrap_or_else(|| panic!("ExtensionRegistry not configured"))
    }

    /// Bind extensions to the session, matching TS bindExtensions().
    /// In the current Rust architecture, extensions are registered at construction
    /// time via ExtensionRegistry, so this is a no-op for the binding logic.
    /// The bindings are stored for reference and the session_start event is emitted.
    pub async fn bind_extensions(&self, bindings: ExtensionBindings) {
        // Store the error listener (matching TS `_extensionErrorListener`).
        if bindings.on_error.is_some() {
            *self
                .extension_error_listener
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = bindings.on_error;
        }
        // In TS, this sets _extensionUIContext, _extensionMode, etc. and calls
        // _applyExtensionBindings() + emits session_start to extensions.
        // In Rust, the equivalent is done at construction time via ExtensionContext.
        // Emit session_start to extensions if any are registered.
        self.emit_session_start("startup").await;
    }

    /// Report an extension error to the bound error listener (matching TS
    /// `ExtensionRunner.emitError`). No-op when no listener is bound.
    pub fn emit_extension_error(&self, extension_path: &str, event: &str, error: &str) {
        if let Some(listener) = self
            .extension_error_listener
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            listener(&ExtensionError {
                extension_path: extension_path.to_string(),
                event: event.to_string(),
                error: error.to_string(),
            });
        }
    }

    /// Emit `session_start` to extensions with the given reason, matching TS
    /// `bindExtensions()` which emits the session start event. Used by RPC
    /// mode's rebind after `new_session` (reason "new") / `switch_session`
    /// (reason "resume"), mirroring TS `rebindSession()`.
    pub async fn emit_session_start(&self, reason: &str) {
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_session_start(
                registry,
                reason,
                &self.ext_ctx,
                None,
            )
            .await;
        }
    }

    /// Create a replaced session context, matching TS createReplacedSessionContext().
    /// Returns a ReplacedSessionContext with send_message and send_user_message methods
    /// bound to this session.
    pub fn create_replaced_session_context(&self) -> ReplacedSessionContext {
        // Bind sendMessage/sendUserMessage to this session (matching TS
        // createReplacedSessionContext which binds sendCustomMessage and
        // sendUserMessage). The closures capture the shared Arc fields and
        // spawn the async work (TS's sendMessage is fire-and-forget too).
        let agent = self.agent.clone();
        let session_manager = self.session_manager.clone();
        let is_agent_run_active = self.is_agent_run_active.clone();
        let pending_next_turn = self.pending_next_turn_messages.clone();
        // Each closure needs its own Arc clones (they are moved into the
        // closures independently).
        let (agent2, sm2, active2, next2) = (
            agent.clone(),
            session_manager.clone(),
            is_agent_run_active.clone(),
            pending_next_turn.clone(),
        );
        ReplacedSessionContext {
            send_message: Some(Box::new(move |message, options| {
                let agent = agent.clone();
                let session_manager = session_manager.clone();
                let is_agent_run_active = is_agent_run_active.clone();
                let pending_next_turn = pending_next_turn.clone();
                tokio::spawn(async move {
                    let opts = options.unwrap_or_default();
                    let timestamp = chrono::Utc::now().timestamp_millis();
                    let app_message = AgentMessage::Custom {
                        custom_type: message.custom_type.clone(),
                        content: pi_agent_core::types::CustomContent::Blocks(
                            message.content.clone(),
                        ),
                        display: message.display.unwrap_or(true),
                        details: message.details.clone(),
                        timestamp,
                    };
                    // deliverAs="nextTurn" → queue for next prompt()
                    if opts.deliver_as.as_deref() == Some("nextTurn") {
                        pending_next_turn
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(app_message);
                        return;
                    }
                    // If streaming, use steer/followUp
                    if *is_agent_run_active
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                    {
                        if opts.deliver_as.as_deref() == Some("followUp") {
                            agent.follow_up(app_message).await;
                        } else {
                            agent.steer(app_message).await;
                        }
                        return;
                    }
                    // Not streaming: persist and optionally trigger a turn
                    session_manager
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .append_custom_message_entry(
                            &message.custom_type,
                            serde_json::to_value(&app_message).unwrap_or_default(),
                            message.display.unwrap_or(true),
                            message.details.clone(),
                        );
                    if opts.trigger_turn {
                        agent.process(vec![app_message]).await.ok();
                    } else {
                        agent
                            .update_state(|s| s.messages.push(app_message))
                            .await;
                    }
                });
            })),
            send_user_message: Some(Box::new(move |content, options| {
                let agent = agent2.clone();
                let session_manager = sm2.clone();
                let is_agent_run_active = active2.clone();
                let pending_next_turn = next2.clone();
                tokio::spawn(async move {
                    let opts = options.unwrap_or_default();
                    let timestamp = chrono::Utc::now().timestamp_millis();
                    let message = AgentMessage::User {
                        content: vec![ContentBlock::text(&content)],
                        timestamp,
                    };
                    if opts.deliver_as.as_deref() == Some("nextTurn") {
                        pending_next_turn
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(message);
                        return;
                    }
                    if *is_agent_run_active
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                    {
                        if opts.deliver_as.as_deref() == Some("followUp") {
                            agent.follow_up(message).await;
                        } else {
                            agent.steer(message).await;
                        }
                        return;
                    }
                    session_manager
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .append_message(serde_json::to_value(&message).unwrap_or_default());
                    agent
                        .update_state(|s| s.messages.push(message))
                        .await;
                });
            })),
        }
    }

    /// Get file-based prompt templates, matching TS `get promptTemplates()`.
    pub fn prompt_templates(&self) -> Vec<crate::core::prompt_templates::PromptTemplate> {
        self.resources
            .as_ref()
            .map(|r| r.prompt_templates.clone())
            .unwrap_or_default()
    }

    pub fn get_steering_messages(&self) -> Vec<String> {
        self.steering_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Get pending follow-up messages (read-only), matching TS getFollowUpMessages().
    pub fn get_follow_up_messages(&self) -> Vec<String> {
        self.follow_up_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Current steering mode, matching TS `get steeringMode()`.
    pub async fn steering_mode(&self) -> QueueMode {
        self.agent.steering_mode().await
    }

    /// Current follow-up mode, matching TS `get followUpMode()`.
    pub async fn follow_up_mode(&self) -> QueueMode {
        self.agent.follow_up_mode().await
    }

    /// Whether compaction or branch summarization is currently running,
    /// matching TS `get isCompacting()`.
    pub fn is_compacting(&self) -> bool {
        self.compaction_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
            || self.auto_compaction_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
            || self.branch_summary_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// Number of pending messages (steering + follow-up), matching TS `get pendingMessageCount()`.
    pub fn pending_message_count(&self) -> usize {
        self.steering_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len() + self.follow_up_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    /// Current retry attempt (0 if not retrying), matching TS `get retryAttempt()`.
    /// Whether a retry is currently in progress, matching TS `get isRetrying()`.
    pub fn is_retrying(&self) -> bool {
        self.retry_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// Whether auto-retry is enabled, matching TS `get autoRetryEnabled()`
    /// which reads settingsManager.getRetryEnabled().
    pub fn auto_retry_enabled(&self) -> bool {
        self.settings_manager
            .lock()
            .map(|sm| sm.get_retry_enabled())
            .unwrap_or(true)
    }

    /// Enable or disable auto-retry, matching TS `setAutoRetryEnabled()`
    /// which writes settingsManager.setRetryEnabled.
    pub fn set_auto_retry_enabled(&self, enabled: bool) {
        if let Ok(mut sm) = self.settings_manager.lock() {
            sm.set_retry_enabled(enabled);
        }
    }

    pub fn retry_attempt(&self) -> u32 {
        *self.retry_attempt.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Get the names of currently active tools, matching TS `getActiveToolNames()`.
    pub async fn get_active_tool_names(&self) -> Vec<String> {
        self.agent
            .state()
            .await
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect()
    }

    /// Get a tool definition by name, matching TS `getToolDefinition()`.
    pub fn get_tool_definition(
        &self,
        name: &str,
    ) -> Option<&crate::core::extensions::ToolDefinition> {
        self.tool_definitions.get(name)
    }

    /// Get all configured tools with name, description, parameter schema,
    /// prompt guidelines, and source metadata, matching TS `getAllTools()`.
    pub fn get_all_tools(&self) -> Vec<crate::core::extensions::ToolInfo> {
        self.tool_definitions
            .values()
            .map(|def| crate::core::extensions::ToolInfo {
                name: def.name.clone(),
                description: def.description.clone(),
                parameters: def.parameters.clone(),
                prompt_guidelines: def.prompt_guidelines.clone(),
                source_info: def.source_info.clone(),
            })
            .collect()
    }

    /// Set active tools by name, matching TS `setActiveToolsByName()`.
    ///
    /// Looks up each name in the full tool registry. Unknown names are
    /// silently ignored. The active tools are immediately reflected on
    /// `agent.state.tools`.
    ///
    /// Note: System prompt rebuild on tool change (as in TS) is not yet
    /// implemented; the tools are available to the LLM but the system
    /// prompt "Available tools" section is not updated dynamically.
    pub async fn set_active_tools_by_name(&self, tool_names: &[String]) {
        let selected: Vec<Arc<pi_agent_core::types::DynTool>> = tool_names
            .iter()
            .filter_map(|name| self.tool_registry.iter().find(|t| t.name == *name).cloned())
            .collect();
        // Write through the shared state — `state()` returns a clone.
        self.agent.update_state(|s| s.tools = selected).await;

        // Rebuild the base system prompt with the new tool set (matching TS
        // setActiveToolsByName which calls _rebuildSystemPrompt).
        if let Some(mut opts) = self.system_prompt_options.clone() {
            opts.selected_tools = Some(tool_names.to_vec());
            let rebuilt = system_prompt::build_system_prompt(&opts);
            *self
                .base_system_prompt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = rebuilt.clone();
            self.agent.set_system_prompt(rebuilt).await;
        }
    }

    // =========================================================================
    // Extension action bus (JS extension actions)
    // =========================================================================

    /// Build the list of discoverable slash commands, matching TS
    /// `getCommands()`. Order: extension commands, prompt templates, skills.
    pub fn get_commands_info(&self) -> Vec<crate::core::slash_commands::SlashCommandInfo> {
        let mut commands: Vec<crate::core::slash_commands::SlashCommandInfo> = Vec::new();

        // Extension commands → source = "extension" (with `:N` dedup).
        if let Some(registry) = self.get_extension_registry() {
            let resolved =
                crate::core::slash_commands::resolve_extension_commands(registry.commands());
            for cmd in resolved {
                commands.push(crate::core::slash_commands::SlashCommandInfo {
                    name: cmd.invocation_name,
                    description: cmd.description,
                    source: crate::core::slash_commands::SlashCommandSource::Extension,
                    source_info: cmd.source_info,
                });
            }
        }

        // Prompt templates → source = "prompt".
        for template in self.prompt_templates() {
            commands.push(crate::core::slash_commands::SlashCommandInfo {
                name: template.name,
                description: Some(template.description),
                source: crate::core::slash_commands::SlashCommandSource::Prompt,
                source_info: template.source_info,
            });
        }

        // Skills → name = "skill:<name>", source = "skill".
        if let Some(resources) = self.resource_loader() {
            for skill in &resources.skills {
                commands.push(crate::core::slash_commands::SlashCommandInfo {
                    name: format!("skill:{}", skill.name),
                    description: Some(skill.description.clone()),
                    source: crate::core::slash_commands::SlashCommandSource::Skill,
                    source_info: skill.source_info.clone(),
                });
            }
        }
        commands
    }

    /// Refresh the shared extension state snapshot from the current session
    /// state. Called at drain points (turn boundaries) so JS extension
    /// read-actions (`getActiveTools`, `getAllTools`, `getSessionName`, …)
    /// see up-to-date values.
    pub async fn refresh_extension_state(&self) {
        let Some(view) = self.extension_state_view.as_ref() else {
            return;
        };
        // Collect all values before taking the lock so no await happens while
        // the MutexGuard is held.
        let model = self.get_model().await;
        let all_tools = self
            .get_all_tools()
            .into_iter()
            .filter_map(|t| serde_json::to_value(t).ok())
            .collect();
        let commands = self
            .get_commands_info()
            .into_iter()
            .filter_map(|c| serde_json::to_value(c).ok())
            .collect();
        let session_name = self.get_session_name();
        let active_tools = self.get_active_tool_names().await;
        let thinking_level = self.get_thinking_level().await;
        let mut guard = view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.session_name = session_name;
        guard.active_tools = active_tools;
        guard.all_tools = all_tools;
        guard.thinking_level = thinking_level;
        guard.commands = commands;
        guard.model_id = Some(format!("{}/{}", model.provider, model.id));
    }

    /// Register a synchronous callback that invalidates the JS extension
    /// runtime when the session changes (new/fork/switch/reload). The SDK
    /// wires this to the extension runtime.
    pub fn set_js_invalidator(&mut self, invalidator: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.js_invalidator = invalidator;
    }

    /// Apply all queued JS extension write-actions, then refresh the state
    /// snapshot. Called at turn boundaries (start of `prompt`/`steer`/
    /// `follow_up`/`send_user_message`).
    ///
    /// Boxed because `send_user_message` → `prompt` → `drain` is a recursive
    /// async cycle (runtime-safe: the queue is fully drained before any action
    /// is applied, so a nested drain is a no-op).
    pub async fn drain_extension_actions(&self) {
        use crate::core::extensions::action_bus::ExtensionAction;
        Box::pin(async move {
            let Some(rx) = self.extension_action_rx.as_ref() else {
                return;
            };
            let mut actions = Vec::new();
            {
                let mut guard = rx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                while let Ok(action) = guard.try_recv() {
                    actions.push(action);
                }
            }
            for action in actions {
                match action {
                    ExtensionAction::SendMessage {
                        custom_type,
                        content,
                        options_json,
                    } => {
                        let opts = options_json
                            .as_deref()
                            .and_then(|o| serde_json::from_str(o).ok());
                        self.send_custom_message(&custom_type, &content, opts).await;
                    }
                    ExtensionAction::SendUserMessage { content, options_json } => {
                        let opts = options_json
                            .as_deref()
                            .and_then(|o| serde_json::from_str(o).ok());
                        if let Err(e) = self.send_user_message(&content, opts).await {
                            // Matches TS sendUserMessage().catch((err) => runner.emitError(...)).
                            eprintln!("[pi] send_user_message failed: {e}");
                        }
                    }
                    ExtensionAction::AppendEntry { custom_type, data_json } => {
                        let data = data_json
                            .as_deref()
                            .and_then(|d| serde_json::from_str(d).ok());
                        self.session_manager
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .append_custom_entry(&custom_type, data);
                    }
                    ExtensionAction::SetSessionName(name) => {
                        self.set_session_name(&name);
                    }
                    ExtensionAction::SetLabel { entry_id, label } => {
                        // `set_label` panics on unknown entries; guard first.
                        let mut mgr = self
                            .session_manager
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if mgr.get_entry(&entry_id).is_some() {
                            mgr.set_label(&entry_id, label.as_deref());
                        }
                    }
                    ExtensionAction::SetActiveTools(tools) => {
                        self.set_active_tools_by_name(&tools).await;
                    }
                    ExtensionAction::SetThinkingLevel(level) => {
                        self.set_thinking_level(&level).await;
                    }
                    ExtensionAction::SetModel(model_id) => {
                        // "provider/model" string, resolved via the model registry.
                        if let Some((provider, id)) = model_id.split_once('/') {
                            if let Some(model) = self.model_registry.find(provider, id) {
                                let _ = self.set_model(model).await;
                            }
                        }
                    }
                }
            }
            self.refresh_extension_state().await;
        })
        .await
    }

    // =========================================================================
    // Session Statistics
    // =========================================================================

    /// Get session statistics, matching the original getSessionStats().
    pub fn get_session_stats(&self) -> SessionStats {
        let mgr = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = mgr.get_entries();
        let (user_messages, assistant_messages, tool_calls, tool_results, total_messages, tokens, cost) =
            compute_session_stats(entries);

        SessionStats {
            session_file: mgr
                .get_session_file()
                .map(|p| p.to_string_lossy().to_string()),
            session_id: mgr.get_session_id().to_string(),
            user_messages,
            assistant_messages,
            tool_calls,
            tool_results,
            total_messages,
            tokens,
            cost,
            ..Default::default()
        }
    }

    // =========================================================================
    // Message Handling
    // =========================================================================

    /// Send a user message to the agent, matching the original prompt() method.
    ///
    /// Refreshes session state from disk before processing the next turn,
    /// ensuring the latest config changes (e.g. tool refresh, session metadata)
    /// are reflected. This aligns with the original TS commit e547bb9.
    ///
    /// After the agent finishes, runs the post-agent-run loop (retry + compaction),
    /// matching TS _runAgentPrompt() + _handlePostAgentRun().
    ///
    /// Returns `Err` when the run cannot start: no model selected or no API key
    /// configured for the model's provider. This mirrors TS prompt() which throws
    /// `formatNoModelSelectedMessage()` / `formatNoApiKeyFoundMessage()` — callers
    /// (RPC/ACP modes) must surface the error to the client instead of reporting a
    /// generic "run failed" failure.
    pub async fn prompt(&self, text: &str, options: Option<PromptOptions>) -> Result<(), String> {
        // Refresh session state before starting the next turn
        if let Err(e) = self
            .session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh_config()
        {
            eprintln!("[pi] Failed to refresh session state before next turn: {e}");
        }
        // Apply queued JS extension actions before the turn starts.
        self.drain_extension_actions().await;

        let opts = options.unwrap_or_default();
        let expand_templates = opts.expand_prompt_templates.unwrap_or(true);
        let source = opts.source.as_deref().unwrap_or("interactive");

        // Handle extension commands first (execute immediately, even during streaming),
        // matching TS prompt() which calls _tryExecuteExtensionCommand(text).
        if expand_templates && text.starts_with("/") && self._try_execute_extension_command(text).await {
            if let Some(cb) = &opts.preflight_result {
                cb(true);
            }
            return Ok(());
        }

        // Emit input event for extension interception (before skill/template expansion),
        // matching TS prompt() which emits input event before expansion.
        let (current_text, current_images) = if let Some(ref registry) = self.extension_registry {
            match crate::core::extensions::dispatcher::dispatch_input(
                crate::core::extensions::dispatcher::DispatchInputParams {
                    registry,
                    text,
                    source,
                    images: opts.images.as_deref(),
                    streaming_behavior: opts.streaming_behavior.as_deref(),
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await
            {
                crate::core::extensions::dispatcher::InputEventResult::Handled => {
                    if let Some(cb) = &opts.preflight_result {
                        cb(true);
                    }
                    return Ok(());
                }
                crate::core::extensions::dispatcher::InputEventResult::Continue {
                    text: t,
                    images,
                } => (t, images),
            }
        } else {
            (text.to_string(), opts.images.clone())
        };

        // Expand skill commands (/skill:name args) and prompt templates (/template args),
        // matching TS _expandSkillCommand() and expandPromptTemplate().
        let expanded_text = if expand_templates {
            let mut t = current_text;
            t = self._expand_skill_command(&t);
            t = crate::core::prompt_templates::expand_prompt_template(&t, &self.prompt_templates());
            t
        } else {
            current_text
        };

        // If streaming, queue via steer() or followUp() based on streamingBehavior option,
        // matching TS prompt() which checks isStreaming. Uses the internal
        // `_queue_steer`/`_queue_follow_up` (pre-expanded text, no extension
        // command check) matching TS prompt() which calls `_queueSteer`/
        // `_queueFollowUp` directly.
        if self.is_streaming().await {
            let behavior = opts.streaming_behavior.as_deref().unwrap_or("steer");
            if behavior == "follow_up" || behavior == "followUp" {
                self._queue_follow_up(&expanded_text, current_images).await;
            } else {
                self._queue_steer(&expanded_text, current_images).await;
            }
            if let Some(cb) = &opts.preflight_result {
                cb(true);
            }
            return Ok(());
        }

        // Flush any pending bash messages before the new prompt,
        // matching TS _flushPendingBashMessages().
        self._flush_pending_bash_messages().await;

        // Validate model and auth before sending,
        // matching TS prompt() which checks model and auth.
        let state = self.agent.state().await;
        if state.model.id.is_empty() {
            if let Some(cb) = &opts.preflight_result {
                cb(false);
            }
            return Err(crate::core::auth_guidance::format_no_model_selected_message(
                &crate::config::get_docs_path().to_string_lossy(),
            ));
        }
        let auth_result = self
            .model_registry
            .get_api_key_and_headers(&state.model)
            .await;
        match auth_result {
            Ok(r) if !r.ok => {
                if let Some(cb) = &opts.preflight_result {
                    cb(false);
                }
                // OAuth providers get a re-auth hint (matching TS prompt()
                // which checks isUsingOAuth before the generic no-key error).
                if self.model_registry.is_using_oauth(&state.model) {
                    return Err(format!(
                        "Authentication failed for \"{}\". Credentials may have expired or network is unavailable. Run '/login {}' to re-authenticate.",
                        state.model.provider, state.model.provider
                    ));
                }
                return Err(crate::core::auth_guidance::format_no_api_key_found_message(
                    &state.model.provider,
                    &crate::config::get_docs_path().to_string_lossy(),
                ));
            }
            Err(e) => {
                if let Some(cb) = &opts.preflight_result {
                    cb(false);
                }
                return Err(format!("Auth check failed: {e}"));
            }
            _ => {}
        }
        drop(state);

        // Check if we need to compact before sending (catches aborted responses),
        // matching TS prompt() which calls _checkCompaction(lastAssistant, false)
        // with the last assistant message (searched from the end, not just the
        // final message which may be a tool result).
        let msgs = self.agent.messages().await;
        if let Some(last_assistant) = msgs
            .iter()
            .rev()
            .find(|m| matches!(m, AgentMessage::Assistant { .. }))
        {
            self._check_compaction(last_assistant, false).await;
        }

        // Send the prompt with pending next-turn messages injected as context,
        // matching TS prompt() which injects _pendingNextTurnMessages.
        // Preflight passed — the prompt is accepted (matching TS
        // `preflightResult?.(true)` right before `_runAgentPrompt`).
        if let Some(cb) = &opts.preflight_result {
            cb(true);
        }
        self.add_user_text_with_options(&expanded_text, current_images, source)
            .await?;

        // Post-agent-run loop: retry + compaction + queued messages
        // Matches TS _runAgentPrompt() which calls _handlePostAgentRun() in a loop.
        loop {
            if !self._handle_post_agent_run().await {
                break;
            }
        }
        // Flush any bash messages queued while the agent was streaming
        // (matching TS _runAgentPrompt's finally block).
        self._flush_pending_bash_messages().await;
        // Emit agent_settled after the agent run is fully complete
        // (no retry, compaction, or queued messages pending).
        self._emit_agent_settled().await;
        Ok(())
    }

    /// Handle post-agent-run tasks: retry, compaction, queued messages.
    /// Returns true if the caller should continue the agent (retry or compaction triggered).
    /// Matches TS _handlePostAgentRun().
    async fn _handle_post_agent_run(&self) -> bool {
        let msg = self.last_assistant_message.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        let msg = match msg {
            Some(m) => m,
            None => return false,
        };

        // Check retry
        if self._is_retryable_error(&msg).await && self._prepare_retry(&msg).await {
            // Continue the agent
            self.agent.continue_run().await.ok();
            return true;
        }

        // Emit auto_retry_end if retry attempt was active but not retryable
        if let AgentMessage::Assistant {
            stop_reason,
            error_message,
            ..
        } = &msg
        {
            if stop_reason == &Some(pi_agent_core::pi_ai_types::StopReason::Error) {
                let retry = *self.retry_attempt.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if retry > 0 {
                    self._emit(AgentSessionEvent::AutoRetryEnd {
                        success: false,
                        attempt: retry,
                        final_error: error_message.clone(),
                    });
                    *self.retry_attempt.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
                }
            }
        }

        // Check compaction
        if self._check_compaction(&msg, true).await {
            return true;
        }

        // Check queued messages
        self.agent.has_queued_messages().await
    }

    /// Try to execute an extension command, matching TS _tryExecuteExtensionCommand().
    /// Returns true if the text was handled as an extension command.
    async fn _try_execute_extension_command(&self, text: &str) -> bool {
        if !text.starts_with("/") {
            return false;
        }
        let space_idx = text.find(' ');
        let command_name = if let Some(idx) = space_idx {
            &text[1..idx]
        } else {
            &text[1..]
        };
        let args = if let Some(idx) = space_idx {
            &text[idx + 1..]
        } else {
            ""
        };

        // Look up the command in the extension registry
        if let Some(ref registry) = self.extension_registry {
            let commands = registry.commands();
            if let Some(cmd) = commands.iter().find(|c| c.name == command_name) {
                (cmd.execute)(args.to_string()).await;
                return true;
            }
        }
        false
    }

    /// Expand skill commands (/skill:name args) to their full content, matching TS _expandSkillCommand().
    /// Returns the expanded text, or the original text if not a skill command or skill not found.
    fn _expand_skill_command(&self, text: &str) -> String {
        if !text.starts_with("/skill:") {
            return text.to_string();
        }

        let space_idx = text.find(' ');
        let skill_name = if let Some(idx) = space_idx {
            &text[7..idx]
        } else {
            &text[7..]
        };
        let args = if let Some(idx) = space_idx {
            text[idx + 1..].trim()
        } else {
            ""
        };

        // Look up the skill in the resource loader
        let skills = self
            .resources
            .as_ref()
            .map(|r| &r.skills[..])
            .unwrap_or(&[]);
        if let Some(skill) = skills.iter().find(|s| s.name == skill_name) {
            match std::fs::read_to_string(&skill.file_path) {
                Ok(content) => {
                    let body = crate::utils::frontmatter::strip_frontmatter(&content)
                        .trim()
                        .to_string();
                    let skill_block = format!(
                        r#"<skill name="{}" location="{}">
References are relative to {}.

{}
</skill>"#,
                        skill.name, skill.file_path, skill.base_dir, body
                    );
                    if args.is_empty() {
                        skill_block
                    } else {
                        format!(
                            "{}

{}",
                            skill_block, args
                        )
                    }
                }
                Err(e) => {
                    // Emit the error like extension commands do (matching TS
                    // _expandSkillCommand which calls emitError).
                    self.emit_extension_error(
                        &skill.file_path,
                        "skill_expansion",
                        &e.to_string(),
                    );
                    text.to_string()
                }
            }
        } else {
            text.to_string()
        }
    }

    /// Flush pending bash messages into agent state, matching TS _flushPendingBashMessages().
    async fn _flush_pending_bash_messages(&self) {
        let messages: Vec<serde_json::Value> =
            std::mem::take(&mut *self.pending_bash_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if messages.is_empty() {
            return;
        }
        for msg_value in &messages {
            if let Ok(agent_msg) = serde_json::from_value::<AgentMessage>(msg_value.clone()) {
                // Write through the shared state — `state()` returns a clone.
                self.agent
                    .update_state(|s| s.messages.push(agent_msg))
                    .await;
                // Note: session persistence is handled by the agent event handler
            }
        }
    }

    /// Send a prompt with options and inject pending next-turn messages as context,
    /// matching TS prompt() which injects _pendingNextTurnMessages alongside the user message.
    async fn add_user_text_with_options(
        &self,
        text: &str,
        images: Option<Vec<ContentBlock>>,
        _source: &str,
    ) -> Result<(), String> {
        *self.is_agent_run_active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;

        // Dispatch before_agent_start to extensions before the agent loop starts.
        // Extensions can cancel the agent start or modify the system prompt.
        if let Some(ref registry) = self.extension_registry {
            let state = self.agent.state().await;
            let images_ref = images.as_deref();
            // Dispatch against the base system prompt (matching TS which
            // passes this._baseSystemPrompt) so extension modifications don't
            // accumulate across turns.
            let base_prompt = self
                .base_system_prompt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let result = crate::core::extensions::dispatcher::dispatch_before_agent_start(
                crate::core::extensions::dispatcher::DispatchBeforeAgentStartParams {
                    registry,
                    system_prompt: &base_prompt,
                    messages: &state.messages,
                    images: images_ref,
                    system_prompt_options: None,
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await;
            if result.cancelled {
                *self.is_agent_run_active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = false;
                return Ok(());
            }
            // Apply the modified system prompt from extensions
            if result.system_prompt != state.system_prompt {
                self.agent.set_system_prompt(result.system_prompt).await;
            }
        }

        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut content = vec![ContentBlock::text(text)];
        if let Some(images) = images {
            content.extend(images);
        }

        // Build messages array: user message + pending next-turn messages as context
        let mut messages = vec![AgentMessage::User { content, timestamp }];

        // Inject any pending "nextTurn" messages as context alongside the user message,
        // matching TS prompt() which injects _pendingNextTurnMessages.
        let pending = std::mem::take(&mut *self.pending_next_turn_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        messages.extend(pending);

        // User message is persisted by the event subscriber on MessageEnd
        if let Ok(mut mgr) = self.session_manager.lock() {
            mgr.set_run_prompt(text);
        }
        // Propagate agent-loop start failures (e.g. agent already busy) instead
        // of swallowing them: TS _runAgentPrompt() lets `agent.prompt()` throw,
        // and the ACP/RPC layer must surface the real error rather than report
        // a generic "run failed" (no AgentEnd) failure.
        self.agent
            .process(messages)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Check if an assistant message has a retryable error.
    /// Context overflow is NOT retryable (handled by compaction instead).
    /// Matches TS _isRetryableError().
    async fn _is_retryable_error(&self, message: &AgentMessage) -> bool {
        let context_window = self.agent.state().await.model.context_window;
        is_retryable_error(message, context_window)
    }

    /// Prepare a retry with exponential backoff.
    /// Returns true if the caller should continue the agent.
    /// Matches TS _prepareRetry().
    async fn _prepare_retry(&self, message: &AgentMessage) -> bool {
        // Read retry settings from settings manager, matching TS
        // settingsManager.getRetrySettings() (enabled + backoff).
        let retry_settings = self.settings_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_retry_settings();
        if !retry_settings.enabled.unwrap_or(true) {
            return false;
        }
        let max_retries = retry_settings.max_retries.unwrap_or(3);
        let base_delay_ms = retry_settings.base_delay_ms.unwrap_or(2000);

        let (retry_count, delay_ms) = {
            let mut retry =
                self.retry_attempt.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            *retry += 1;

            if *retry > max_retries {
                *retry -= 1;
                return false;
            }

            (*retry, base_delay_ms * 2u64.pow(*retry - 1))
        };
        let error_message = if let AgentMessage::Assistant { error_message, .. } = message {
            error_message
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string())
        } else {
            "Unknown error".to_string()
        };

        self._emit(AgentSessionEvent::AutoRetryStart {
            attempt: retry_count,
            max_attempts: max_retries,
            delay_ms,
            error_message: error_message.clone(),
        });

        // Remove error message from agent state (keep in session for history),
        // matching TS _prepareRetry().
        let msgs = self.agent.messages().await;
        if msgs.last().map(|m| m.role()) == Some("assistant") {
            let mut truncated = msgs;
            truncated.pop();
            self.agent.set_initial_messages(truncated).await;
        }

        // Create abort controller for retry backoff, matching TS `this._retryAbortController = new AbortController()`
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        *self.retry_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);

        // Wait for backoff with abort support, matching TS await with AbortController
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {
                // Backoff completed normally
                *self.retry_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            _ = rx.changed() => {
                // Retry was aborted — emit the end event so the UI can clean
                // up (matching TS _prepareRetry's catch branch).
                *self.retry_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                let attempt = *self.retry_attempt.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                *self.retry_attempt.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
                self._emit(AgentSessionEvent::AutoRetryEnd {
                    success: false,
                    attempt,
                    final_error: Some("Retry cancelled".to_string()),
                });
                return false;
            }
        }

        true
    }

    /// Check if compaction is needed and run it.
    /// Called after agent_end and before prompt submission.
    /// Matches TS _checkCompaction().
    async fn _check_compaction(
        &self,
        assistant_message: &AgentMessage,
        skip_aborted_check: bool,
    ) -> bool {
        // Check compaction settings
        if !self.compaction_settings.compact_on_threshold {
            return false;
        }

        if let AgentMessage::Assistant {
            stop_reason,
            provider,
            model,
            ..
        } = assistant_message
        {
            // Skip if message was aborted (user cancelled) - unless skip_aborted_check is false
            if skip_aborted_check
                && stop_reason == &Some(pi_agent_core::pi_ai_types::StopReason::Aborted)
            {
                return false;
            }

            // Skip overflow check if the message came from a different model.
            // This handles the case where user switched from a smaller-context model
            // to a larger-context model - the overflow error from the old model
            // should not trigger compaction for the new model.
            let state = self.agent.state().await;
            let same_model = state.model.provider == *provider && state.model.id == *model;

            // Skip compaction checks if this assistant message is older than the latest
            // compaction boundary. This prevents a stale pre-compaction usage/error
            // from retriggering compaction on the first prompt after compaction.
            let branch_entries = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_branch(None);
            let latest_compaction_ts = branch_entries.iter().rev().find_map(|e| {
                if let crate::core::session_manager::SessionEntry::Compaction {
                    timestamp, ..
                } = e
                {
                    Some(timestamp.clone())
                } else {
                    None
                }
            });
            if let Some(ref compaction_ts) = latest_compaction_ts {
                if let AgentMessage::Assistant {
                    timestamp: msg_ts, ..
                } = assistant_message
                {
                    if *msg_ts as f64 <= compaction_ts.parse::<f64>().unwrap_or(0.0) {
                        return false;
                    }
                }
            }

            if same_model {
                let context_window = state.model.context_window;
                // Check for context overflow (matching TS isContextOverflow:
                // error-message patterns, silent usage overflow, and
                // length-stop overflow).
                if context_window > 0 {
                    let is_overflow = match assistant_message {
                        AgentMessage::Assistant {
                            content,
                            api,
                            provider,
                            model,
                            usage,
                            stop_reason,
                            error_message,
                            timestamp,
                        } => {
                            let msg = pi_agent_core::pi_ai_types::AssistantMessage {
                                content: content.clone(),
                                api: api.clone(),
                                provider: provider.clone(),
                                model: model.clone(),
                                response_model: None,
                                response_id: None,
                                diagnostics: None,
                                usage: usage.clone(),
                                stop_reason: stop_reason.clone().unwrap_or(
                                    pi_agent_core::pi_ai_types::StopReason::Stop,
                                ),
                                error_message: error_message.clone(),
                                raw_stop_reason: None,
                                timestamp: *timestamp,
                            };
                            pi_agent_core::pi_ai::utils::overflow::is_context_overflow(
                                &msg,
                                Some(context_window),
                            )
                        }
                        _ => false,
                    };

                    if is_overflow {
                        let will_retry = stop_reason
                            != &Some(pi_agent_core::pi_ai_types::StopReason::Stop);

                        if !will_retry {
                            return self._run_auto_compaction("overflow", false).await;
                        }

                        if *self.overflow_recovery_attempted.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
                            self._emit(AgentSessionEvent::CompactionEnd {
                                reason: CompactionReason::Overflow,
                                result: None,
                                aborted: false,
                                will_retry: false,
                                error_message: Some(
                                    "Context overflow recovery failed after one compact-and-retry attempt. \
                                     Try reducing context or switching to a larger-context model."
                                        .to_string(),
                                ),
                            });
                            return false;
                        }

                        *self.overflow_recovery_attempted.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                        // Remove the error message from agent state
                        let msgs = self.agent.messages().await;
                        if msgs.last().map(|m| m.role()) == Some("assistant") {
                            let mut truncated = msgs;
                            truncated.pop();
                            self.agent.set_initial_messages(truncated).await;
                        }
                        return self._run_auto_compaction("overflow", will_retry).await;
                    }
                }
            }
        }

        // Threshold-based compaction
        let total_tokens = self.check_auto_compact().await;
        if total_tokens {
            return self._run_auto_compaction("threshold", false).await;
        }

        false
    }

    /// Run auto-compaction with events.
    /// Matches TS _runAutoCompaction().
    async fn _run_auto_compaction(&self, reason: &str, will_retry: bool) -> bool {
        let compaction_reason = match reason {
            "overflow" => CompactionReason::Overflow,
            "threshold" => CompactionReason::Threshold,
            _ => CompactionReason::Threshold,
        };

        // compaction_start is emitted inside compact() (matching TS: manual
        // compact() emits it at the top; _runAutoCompaction emits it after
        // preparation succeeds, which compact() re-checks).

        // Create abort signal
        let (tx, rx) = tokio::sync::watch::channel(false);
        *self.auto_compaction_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);

        // Run compaction
        match self.compact(None).await {
            Ok(_summary) => {
                // Check if aborted
                if *rx.borrow() {
                    self._emit(AgentSessionEvent::CompactionEnd {
                        reason: compaction_reason,
                        result: None,
                        aborted: true,
                        will_retry: false,
                        error_message: None,
                    });
                    return false;
                }

                self._emit(AgentSessionEvent::CompactionEnd {
                    reason: compaction_reason,
                    result: None,
                    aborted: false,
                    will_retry,
                    error_message: None,
                });
                true
            }
            Err(e) => {
                if e == "Compaction not needed" {
                    return false;
                }
                // An extension cancelling compaction is an abort, not a failure
                // (matching TS _runAutoCompaction which emits aborted: true).
                let aborted = e == "Compaction cancelled by extension";
                self._emit(AgentSessionEvent::CompactionEnd {
                    reason: compaction_reason,
                    result: None,
                    aborted,
                    will_retry: false,
                    error_message: if aborted {
                        None
                    } else {
                        Some(format!("Compaction failed: {e}"))
                    },
                });
                false
            }
        }
    }

    pub async fn add_user_message(&mut self, mut content: Vec<ContentBlock>) {
        // Normalize empty content at ingestion boundary
        if content.is_empty() {
            content = vec![ContentBlock::Text {
                text: String::new(),
                text_signature: None,
            }];
        }

        // Used by the interaction loop to preserve prompt through session refresh
        let text: String = content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text, .. } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<&str>>()
            .join("\n");

        // Dispatch before_agent_start to extensions before the agent loop starts.
        // Extensions can cancel the agent start or modify the system prompt.
        if let Some(ref registry) = self.extension_registry {
            let state = self.agent.state().await;
            // Extract images from content blocks for extension dispatch
            let images: Vec<ContentBlock> = content
                .iter()
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .cloned()
                .collect();
            let images_ref = if images.is_empty() {
                None
            } else {
                Some(images.as_slice())
            };
            let base_prompt = self
                .base_system_prompt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let result = crate::core::extensions::dispatcher::dispatch_before_agent_start(
                crate::core::extensions::dispatcher::DispatchBeforeAgentStartParams {
                    registry,
                    system_prompt: &base_prompt,
                    messages: &state.messages,
                    images: images_ref,
                    system_prompt_options: None,
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await;
            if result.cancelled {
                return;
            }
            // Apply the modified system prompt from extensions
            if result.system_prompt != state.system_prompt {
                self.agent.set_system_prompt(result.system_prompt).await;
            }
        }

        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User { content, timestamp };
        // User message is persisted by the event subscriber on MessageEnd
        if let Ok(mut mgr) = self.session_manager.lock() {
            mgr.set_run_prompt(&text);
        }
        self.agent.process(vec![message]).await.ok();
    }

    pub async fn add_user_text(&self, text: &str) {
        *self.is_agent_run_active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        // Dispatch input event to extensions before processing.
        // If an extension handles the input, skip processing entirely.
        // If an extension transforms the text, use the transformed text.
        let (effective_text, effective_images) = if let Some(ref registry) = self.extension_registry
        {
            match crate::core::extensions::dispatcher::dispatch_input(
                crate::core::extensions::dispatcher::DispatchInputParams {
                    registry,
                    text,
                    source: "interactive",
                    images: None,
                    streaming_behavior: None,
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await
            {
                crate::core::extensions::dispatcher::InputEventResult::Handled => return,
                crate::core::extensions::dispatcher::InputEventResult::Continue {
                    text: t,
                    images,
                } => (t, images),
            }
        } else {
            (text.to_string(), None)
        };

        // Dispatch before_agent_start to extensions before the agent loop starts.
        // Extensions can cancel the agent start or modify the system prompt.
        if let Some(ref registry) = self.extension_registry {
            let state = self.agent.state().await;
            let base_prompt = self
                .base_system_prompt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let result = crate::core::extensions::dispatcher::dispatch_before_agent_start(
                crate::core::extensions::dispatcher::DispatchBeforeAgentStartParams {
                    registry,
                    system_prompt: &base_prompt,
                    messages: &state.messages,
                    images: None,
                    system_prompt_options: None,
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await;
            if result.cancelled {
                *self.is_agent_run_active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = false;
                return;
            }
            // Apply the modified system prompt from extensions
            if result.system_prompt != state.system_prompt {
                self.agent.set_system_prompt(result.system_prompt).await;
            }
        }

        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut content = vec![ContentBlock::text(&effective_text)];
        if let Some(images) = effective_images {
            content.extend(images);
        }
        let message = AgentMessage::User { content, timestamp };
        // User message is persisted by the event subscriber on MessageEnd
        if let Ok(mut mgr) = self.session_manager.lock() {
            mgr.set_run_prompt(&effective_text);
        }
        self.agent.process(vec![message]).await.ok();
    }

    // =========================================================================
    // Model Management
    // =========================================================================

    /// Set the model on the agent, matching the original setModel().
    /// Persists to session and settings, re-clamps thinking level, and emits events.
    pub async fn set_model(&self, model: Model) -> Result<(), String> {
        // Check auth before setting model, matching TS _modelRuntime.checkAuth()
        let auth_result = self.model_registry.get_api_key_and_headers(&model).await?;
        if !auth_result.ok {
            return Err(format!(
                "No API key configured for provider '{}'. Set the appropriate environment variable or configure it via /login.",
                model.provider
            ));
        }

        let model_id = model.id.clone();
        let model_provider = model.provider.clone();
        let state = self.agent.state().await;
        let previous_model_id = state.model.id.clone();
        let previous_model = if previous_model_id.is_empty() {
            None
        } else {
            Some(previous_model_id.clone())
        };
        // Write through the shared state — `state()` returns a clone.
        self.agent.set_model(model).await;

        // Persist to session (matching TS sessionManager.appendModelChange)
        self.session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append_model_change(&model_provider, &model_id);

        // Persist to settings (matching TS settingsManager.setDefaultModelAndProvider)
        if let Ok(mut sm) = self.settings_manager.lock() {
            sm.set_default_model_and_provider(&model_provider, &model_id);
        }

        // Re-clamp thinking level for new model's capabilities (matching TS setThinkingLevel call)
        self.set_thinking_level(&self._get_thinking_level_for_model_switch(None).await)
            .await;

        // Dispatch model_select to extensions (matching TS _emitModelSelect)
        // Note: TS only sends model_select to extension runner, not as session event
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_model_select(
                crate::core::extensions::dispatcher::DispatchModelSelectParams {
                    registry,
                    model: &model_id,
                    previous_model: previous_model.as_deref(),
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await;
        }
        Ok(())
    }

    /// Get the thinking level to use when switching models, matching TS _getThinkingLevelForModelSwitch().
    async fn _get_thinking_level_for_model_switch(&self, explicit_level: Option<&str>) -> String {
        if let Some(level) = explicit_level {
            return level.to_string();
        }
        if !self.supports_thinking().await {
            return self
                .settings_manager
                .lock()
                .map(|sm| {
                    sm.get_default_thinking_level()
                        .unwrap_or("medium")
                        .to_string()
                })
                .unwrap_or_else(|_| "medium".to_string());
        }
        self.agent.state().await.thinking_level
    }

    /// Set the thinking level on the agent.
    /// Clamps to model capabilities, matching TS setThinkingLevel().
    /// Persists to session and settings, emits events, dispatches to extensions.
    pub async fn set_thinking_level(&self, level: &str) {
        let state = self.agent.state().await;
        let available = pi_agent_core::pi_ai_types::get_supported_thinking_levels(&state.model);
        let effective = if available.contains(&level) {
            level.to_string()
        } else {
            pi_agent_core::pi_ai_types::clamp_thinking_level(&state.model, level)
        };
        let previous_level = state.thinking_level.clone();
        let is_changing = effective != previous_level;
        drop(state);

        if is_changing {
            // Write through the shared state — `state()` returns a clone.
            self.agent.set_thinking_level(effective.clone()).await;

            // Persist to session (matching TS sessionManager.appendThinkingLevelChange)
            self.session_manager
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .append_thinking_level_change(&effective);

            // Persist to settings (matching TS settingsManager.setDefaultThinkingLevel)
            if self.supports_thinking().await || effective != "off" {
                if let Ok(mut sm) = self.settings_manager.lock() {
                    sm.set_default_thinking_level(&effective);
                }
            }

            // Emit thinking_level_changed event (matching TS _emit)
            self._emit(AgentSessionEvent::ThinkingLevelChanged {
                level: effective.clone(),
            });

            // Dispatch thinking_level_select to extensions
            if let Some(ref registry) = self.extension_registry {
                crate::core::extensions::dispatcher::dispatch_thinking_level_select(
                    crate::core::extensions::dispatcher::DispatchThinkingLevelSelectParams {
                        registry,
                        level: &effective,
                        previous_level: &previous_level,
                        ext_ctx: &self.ext_ctx,
                    },
                )
                .await;
            }
        }
    }

    /// Get available thinking levels for the current model, matching TS.
    pub async fn get_available_thinking_levels(&self) -> Vec<&'static str> {
        let model = self.agent.state().await.model;
        pi_agent_core::pi_ai_types::get_supported_thinking_levels(&model)
    }

    /// Check if the current model supports thinking/reasoning, matching TS.
    pub async fn supports_thinking(&self) -> bool {
        self.agent.state().await.model.reasoning
    }

    /// Cycle to the next thinking level, matching TS cycleThinkingLevel().
    /// Returns the new level, or None if the model doesn't support thinking.
    pub async fn cycle_thinking_level(&mut self) -> Option<String> {
        if !self.supports_thinking().await {
            return None;
        }
        let levels = self.get_available_thinking_levels().await;
        let current = self.agent.state().await.thinking_level;
        // TS uses levels.indexOf(this.thinkingLevel): -1 (not found) wraps to
        // the first level via (-1 + 1) % len == 0.
        let next_idx = match levels.iter().position(|&l| l == current) {
            Some(i) => (i + 1) % levels.len(),
            None => 0,
        };
        let next = levels[next_idx].to_string();
        self.set_thinking_level(&next).await;
        Some(next)
    }

    /// Cycle through scoped models, matching the original cycleModel().
    /// Sets the model on the agent, persists to session and settings, re-clamps thinking level.
    /// Returns the new model and thinking level, and whether it's a scoped model.
    pub async fn cycle_model(
        &mut self,
        direction: &str,
    ) -> Option<(Model, Option<ThinkingLevel>, bool)> {
        // Scoped models (from --models flag) take precedence, otherwise cycle
        // through all available models (matching TS cycleModel()).
        if !self.scoped_models.is_empty() {
            return self._cycle_scoped_model(direction).await;
        }
        self._cycle_available_model(direction).await
    }

    /// Cycle through scoped models, matching TS `_cycleScopedModel()`.
    async fn _cycle_scoped_model(
        &mut self,
        direction: &str,
    ) -> Option<(Model, Option<ThinkingLevel>, bool)> {
        if self.scoped_models.is_empty() {
            return None;
        }

        let current_model = self.agent.state().await.model;
        let current_idx = self
            .scoped_models
            .iter()
            .position(|(m, _)| m.provider == current_model.provider && m.id == current_model.id);

        let new_idx = match (current_idx, direction) {
            (Some(i), "forward") => (i + 1) % self.scoped_models.len(),
            (Some(i), "backward") => {
                if i == 0 {
                    self.scoped_models.len() - 1
                } else {
                    i - 1
                }
            }
            (None, _) | (_, _) => 0,
        };

        let (model, thinking_level) = self.scoped_models[new_idx].clone();
        let model_id = model.id.clone();
        let model_provider = model.provider.clone();

        // Set model on agent state (matching TS _cycleScopedModel)
        let state = self.agent.state().await;
        let previous_model_id = state.model.id.clone();
        let previous_model = if previous_model_id.is_empty() {
            None
        } else {
            Some(previous_model_id.clone())
        };
        // Write through the shared state — `state()` returns a clone.
        self.agent.set_model(model.clone()).await;

        // Persist to session (matching TS sessionManager.appendModelChange)
        self.session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append_model_change(&model_provider, &model_id);

        // Persist to settings (matching TS settingsManager.setDefaultModelAndProvider)
        if let Ok(mut sm) = self.settings_manager.lock() {
            sm.set_default_model_and_provider(&model_provider, &model_id);
        }

        // Apply thinking level (matching TS setThinkingLevel call in _cycleScopedModel)
        let tl = self
            ._get_thinking_level_for_model_switch(thinking_level.as_deref())
            .await;
        self.set_thinking_level(&tl).await;
        // TS returns this.thinkingLevel (the clamped current value).
        let effective_tl = self.agent.state().await.thinking_level;

        // Dispatch model_select to extensions (matching TS _emitModelSelect)
        // Note: TS only sends model_select to extension runner, not as session event
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_model_select(
                crate::core::extensions::dispatcher::DispatchModelSelectParams {
                    registry,
                    model: &model_id,
                    previous_model: previous_model.as_deref(),
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await;
        }

        Some((model, Some(effective_tl), true))
    }

    /// Cycle through all available models, matching TS `_cycleAvailableModel()`.
    async fn _cycle_available_model(
        &mut self,
        direction: &str,
    ) -> Option<(Model, Option<ThinkingLevel>, bool)> {
        let available_models = self.model_registry.get_models();
        if available_models.len() <= 1 {
            return None;
        }

        let current_model = self.agent.state().await.model;
        let current_idx = available_models
            .iter()
            .position(|m| m.provider == current_model.provider && m.id == current_model.id)
            .unwrap_or(0);
        let len = available_models.len();
        let next_idx = match direction {
            "forward" => (current_idx + 1) % len,
            "backward" => (current_idx + len - 1) % len,
            _ => (current_idx + 1) % len,
        };
        let next_model = available_models[next_idx].clone();
        let model_id = next_model.id.clone();
        let model_provider = next_model.provider.clone();
        let previous_model_id = current_model.id.clone();
        let previous_model = if previous_model_id.is_empty() {
            None
        } else {
            Some(previous_model_id)
        };

        // Apply model (matching TS _cycleAvailableModel)
        self.agent.set_model(next_model.clone()).await;
        self.session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append_model_change(&model_provider, &model_id);
        if let Ok(mut sm) = self.settings_manager.lock() {
            sm.set_default_model_and_provider(&model_provider, &model_id);
        }

        // Re-clamp thinking level for the new model's capabilities.
        let tl = self._get_thinking_level_for_model_switch(None).await;
        self.set_thinking_level(&tl).await;
        let effective_tl = self.agent.state().await.thinking_level;

        // Dispatch model_select to extensions (matching TS _emitModelSelect)
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_model_select(
                crate::core::extensions::dispatcher::DispatchModelSelectParams {
                    registry,
                    model: &model_id,
                    previous_model: previous_model.as_deref(),
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await;
        }

        Some((next_model, Some(effective_tl), false))
    }

    // =========================================================================
    // Compaction
    // =========================================================================

    /// Check whether compaction should be triggered, matching the original shouldCompact().
    pub fn check_should_compact(&self, total_tokens: u64, context_window: u64) -> bool {
        use crate::core::compaction;
        compaction::should_compact(total_tokens, context_window, &self.compaction_settings)
    }

    /// Check whether compaction should be triggered, using token estimation.
    /// Returns true if the context is above the threshold.
    pub async fn check_auto_compact(&self) -> bool {
        use crate::core::compaction;

        let state = self.agent.state().await;
        let messages = state.messages;
        let total_tokens = compaction::estimate_agent_messages_tokens(&messages);
        let context_window = state.model.context_window.max(1);

        compaction::should_compact(total_tokens, context_window, &self.compaction_settings)
    }

    /// Trigger compaction, matching the original compact().
    /// Returns a summary string on success.
    pub async fn compact(
        &self,
        custom_instructions: Option<&str>,
    ) -> Result<crate::core::compaction::CompactionResult, String> {
        use crate::core::compaction;

        // Abort any in-progress agent run before compacting (matching TS
        // compact() which calls this.abort() first).
        self.abort().await;

        // Dispatch session_before_compact to extensions.
        // If an extension cancels, return early.
        if let Some(ref registry) = self.extension_registry {
            let cancelled = crate::core::extensions::dispatcher::dispatch_session_before_compact(
                registry,
                if custom_instructions.is_some() {
                    "manual"
                } else {
                    "auto"
                },
                false,
                &self.ext_ctx,
            )
            .await;
            if cancelled {
                return Err("Compaction cancelled by extension".to_string());
            }
        }

        let state = self.agent.state().await;
        let messages = state.messages;
        let total_tokens = compaction::estimate_agent_messages_tokens(&messages);
        let context_window = state.model.context_window.max(1);

        if !compaction::should_compact(total_tokens, context_window, &self.compaction_settings) {
            return Err("Compaction not needed".to_string());
        }

        // Emit compaction_start once the run is actually going to happen
        // (matching TS: manual compact() emits it at the top; auto-compaction
        // emits it after preparation succeeds). Emitting only after the
        // should_compact check avoids a dangling compaction_start when
        // compaction is not needed.
        self._emit(AgentSessionEvent::CompactionStart {
            reason: CompactionReason::Manual,
        });

        let keep_recent_turns = 5usize;
        let cut_point = compaction::find_compaction_cut_point(&messages, keep_recent_turns);

        let prepared = compaction::prepare_compaction(
            &messages,
            keep_recent_turns,
            self.compaction_settings.clone(),
        );

        // Build the summarization prompt
        let summarization_prompt = compaction::build_summarization_prompt(
            &prepared.messages_to_summarize,
            prepared.previous_summary.as_deref(),
            custom_instructions,
        );

        // Generate summary using the LLM if a stream_fn is available.
        // Wrapped in retryAssistantCall (match TS `completeSummarization`) so
        // transient stream drops honor the configured retry policy.
        let summary = if let Some(stream_fn) = self.agent.get_stream_fn() {
            let model = self.agent.state().await.model;
            let llm_context = pi_agent_core::pi_ai_types::Context {
                system_prompt: Some(compaction::SUMMARIZATION_SYSTEM_PROMPT.to_string()),
                messages: vec![pi_agent_core::pi_ai_types::Message::User {
                    content: vec![pi_agent_core::pi_ai_types::ContentBlock::text(
                        &summarization_prompt,
                    )],
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }],
                tools: None,
            };

            // Retry policy from settings (match TS settingsManager.getRetrySettings())
            let retry_settings = self
                .settings_manager
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_retry_settings();
            let retry_policy = pi_agent_core::pi_ai_types::RetryPolicy {
                enabled: retry_settings.enabled.unwrap_or(true),
                max_retries: retry_settings.max_retries.unwrap_or(0),
                base_delay_ms: retry_settings.base_delay_ms.unwrap_or(1000),
            };
            let callbacks = self._summarization_retry_callbacks(
                "compaction",
                Some(if custom_instructions.is_some() {
                    "manual".to_string()
                } else {
                    "threshold".to_string()
                }),
            );

            let produce = || {
                let stream_fn = stream_fn.clone();
                let model = model.clone();
                let llm_context = llm_context.clone();
                async move {
                    // Summaries are standalone requests: fresh routing session ID
                    // + cacheRetention "none" (match TS #6618).
                    let fn_options = pi_agent_core::types::StreamFnOptions {
                        cache_retention: Some(pi_agent_core::pi_ai_types::CacheRetention::None),
                        session_id: Some(pi_agent_core::pi_ai_types::uuid_v7()),
                        ..Default::default()
                    };
                    consume_stream_to_message(stream_fn, model, llm_context, fn_options).await
                }
            };
            let result = pi_agent_core::pi_ai_types::retry_assistant_call(
                produce,
                Some(retry_policy),
                None,
                Some(&callbacks),
            )
            .await;

            // Extract text from the final message
            let mut full_text = String::new();
            for block in &result.content {
                if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = block {
                    full_text.push_str(text);
                }
            }
            if full_text.trim().is_empty() {
                format!(
                    "Compacted {} messages (summary generation unavailable)",
                    messages.len()
                )
            } else {
                full_text
            }
        } else {
            format!("Compacted {} messages", messages.len())
        };

        // Record compaction in session manager
        {
            let mut mgr = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            mgr.append_compaction(
                &summary,
                &cut_point.first_kept_entry_index.to_string(),
                total_tokens,
                None,
                None,
                None,
            );
        }

        // Dispatch session_compact to extensions after compaction.
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_session_compact(
                crate::core::extensions::dispatcher::DispatchSessionCompactParams {
                    registry,
                    summary: &summary,
                    tokens_before: total_tokens,
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await;
        }

        Ok(crate::core::compaction::CompactionResult {
            summary,
            first_kept_entry_id: cut_point.first_kept_entry_index.to_string(),
            tokens_before: total_tokens,
            estimated_tokens_after: None,
            details: Some(crate::core::compaction::CompactionDetails::default()),
        })
    }

    // =========================================================================
    // Tree Navigation
    // =========================================================================

    /// Navigate the session tree, matching the original navigateTree().
    /// `direction` can be "up", "down", "root", or an entry ID.
    pub async fn navigate_tree(&mut self, direction: &str) -> bool {
        // Dispatch session_before_tree event to extensions
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_session_before_tree(
                registry,
                direction,
                &self.ext_ctx,
            )
            .await;
        }

        let result = {
            let mut mgr =
                self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match direction {
                "up" | "parent" => mgr.navigate_to_parent(),
                "root" => {
                    // Navigate to the first entry (root)
                    let first_id = mgr.get_entries().first().map(|e| e.id().to_string());
                    if let Some(id) = first_id {
                        mgr.navigate_to(&id)
                    } else {
                        false
                    }
                }
                _ => {
                    // Treat as an entry ID
                    mgr.navigate_to(direction)
                }
            }
        };

        // Dispatch session_tree event to extensions (fire-and-forget)
        if let Some(ref registry) = self.extension_registry {
            registry.hook_runner().fire_tree(None, None).await;
        }

        result
    }

    /// Get the session tree, matching the original getTree().
    pub fn get_tree(&self) -> Vec<crate::core::session_manager::SessionTreeNode> {
        self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_tree()
    }

    // =========================================================================
    // Custom Messages
    // =========================================================================

    /// Send a custom message (for extensions), matching the original sendCustomMessage().
    /// Send a custom message, matching TS sendCustomMessage().
    /// Supports `triggerTurn` and `deliverAs` options.
    pub async fn send_custom_message(
        &self,
        custom_type: &str,
        content: &str,
        options: Option<CustomMessageOptions>,
    ) {
        let opts = options.unwrap_or_default();
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User {
            content: vec![ContentBlock::text(content)],
            timestamp,
        };

        // deliverAs="nextTurn" → queue for next prompt()
        if opts.deliver_as.as_deref() == Some("nextTurn") {
            self.pending_next_turn_messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(message);
            return;
        }

        // If streaming, use steer/followUp
        if *self.is_agent_run_active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            if opts.deliver_as.as_deref() == Some("followUp") {
                self.agent.follow_up(message).await;
            } else {
                self.agent.steer(message).await;
            }
            return;
        }

        // Not streaming: persist and optionally trigger a turn
        self.session_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append_custom_message_entry(
                custom_type,
                serde_json::to_value(&message).unwrap_or_default(),
                true,
                None,
            );

        if opts.trigger_turn {
            self.agent.process(vec![message]).await.ok();
        } else {
            // Just append to messages and emit events. Write through the
            // shared state — `state()` returns a clone.
            self.agent
                .update_state(|s| s.messages.push(message))
                .await;
        }
    }

    // =========================================================================
    // Streaming Queue Management
    // =========================================================================

    /// Queue a steering message (interrupts current stream), matching TS steer().
    /// Supports optional image attachments.
    /// Queue a steering message (interrupts the current stream), matching TS
    /// `steer()`. Errors if the text is an extension command (cannot be
    /// queued, matching TS `_throwIfExtensionCommand`). Expands skill commands
    /// and prompt templates before queueing.
    pub async fn steer(&self, text: &str, images: Option<Vec<ContentBlock>>) -> Result<(), String> {
        if text.starts_with('/') {
            self._throw_if_extension_command(text)?;
        }
        let expanded_text = self._expand_skill_command(text);
        let expanded_text =
            crate::core::prompt_templates::expand_prompt_template(&expanded_text, &self.prompt_templates());
        self._queue_steer(&expanded_text, images).await;
        Ok(())
    }

    /// Queue a follow-up message (waits for the current stream), matching TS
    /// `followUp()`. Errors on extension commands; expands skills/templates.
    pub async fn follow_up(&self, text: &str, images: Option<Vec<ContentBlock>>) -> Result<(), String> {
        if text.starts_with('/') {
            self._throw_if_extension_command(text)?;
        }
        let expanded_text = self._expand_skill_command(text);
        let expanded_text =
            crate::core::prompt_templates::expand_prompt_template(&expanded_text, &self.prompt_templates());
        self._queue_follow_up(&expanded_text, images).await;
        Ok(())
    }

    /// Internal: enqueue an already-expanded steering message (matching TS
    /// `_queueSteer`, which receives pre-expanded text and skips the extension
    /// command check — the caller has already handled commands).
    async fn _queue_steer(&self, text: &str, images: Option<Vec<ContentBlock>>) {
        // Mirror the queued message for pendingMessageCount / getSteeringMessages
        // / QueueUpdate events (matching TS _queueSteer which pushes to
        // `_steeringMessages` and emits a queue update).
        self.steering_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(text.to_string());
        self._emit_queue_update();
        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut content = vec![ContentBlock::text(text)];
        if let Some(imgs) = images {
            content.extend(imgs);
        }
        let message = AgentMessage::User { content, timestamp };
        self.agent.steer(message).await;
    }

    /// Internal: enqueue an already-expanded follow-up message (matching TS
    /// `_queueFollowUp`).
    async fn _queue_follow_up(&self, text: &str, images: Option<Vec<ContentBlock>>) {
        self.follow_up_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(text.to_string());
        self._emit_queue_update();
        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut content = vec![ContentBlock::text(text)];
        if let Some(imgs) = images {
            content.extend(imgs);
        }
        let message = AgentMessage::User { content, timestamp };
        self.agent.follow_up(message).await;
    }

    /// Throw if the text is an extension command (cannot be queued), matching
    /// TS `_throwIfExtensionCommand`.
    fn _throw_if_extension_command(&self, text: &str) -> Result<(), String> {
        if !text.starts_with('/') {
            return Ok(());
        }
        let space_idx = text.find(' ');
        let command_name = if let Some(idx) = space_idx {
            &text[1..idx]
        } else {
            &text[1..]
        };
        if let Some(ref registry) = self.extension_registry {
            let commands = registry.commands();
            if commands.iter().any(|c| c.name == command_name) {
                return Err(format!(
                    "Extension command \"/{command_name}\" cannot be queued. Use prompt() or execute the command when not streaming."
                ));
            }
        }
        Ok(())
    }

    /// Check if there are queued messages, matching original hasQueuedMessages().
    pub async fn has_queued_messages(&self) -> bool {
        self.agent.has_queued_messages().await
    }

    /// Clear all queued messages, matching original clearAllQueues().
    /// Returns the cleared messages, matching TS clearQueue().
    pub async fn clear_all_queues(&self) -> (Vec<String>, Vec<String>) {
        let steering = self
            .steering_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect::<Vec<_>>();
        let follow_up = self
            .follow_up_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect::<Vec<_>>();
        self.agent.clear_all_queues().await;
        self._emit_queue_update();
        (steering, follow_up)
    }

    /// Retry the last turn, matching original retry().
    /// Returns the new messages on success.
    pub async fn retry(
        &self,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        self.agent.continue_run().await
    }

    /// Abort in-progress retry, matching TS abortRetry().
    pub fn abort_retry(&self) {
        // Abort the retry backoff, matching TS abortRetry() which calls `this._retryAbortController?.abort()`.
        if let Some(sender) = self.retry_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            let _ = sender.send(true);
        }
        // Reset retry state
        *self.retry_attempt.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
    }

    /// Cancel in-progress compaction (manual or auto), matching TS abortCompaction().
    pub fn abort_compaction(&self) {
        if let Some(sender) = self.compaction_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            let _ = sender.send(true);
        }
        if let Some(sender) = self.auto_compaction_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            let _ = sender.send(true);
        }
    }

    /// Cancel in-progress branch summarization, matching TS abortBranchSummary().
    pub fn abort_branch_summary(&self) {
        if let Some(sender) = self.branch_summary_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            let _ = sender.send(true);
        }
    }

    // =========================================================================
    // Export
    // =========================================================================

    /// Export the session as HTML, matching the original exportHTML().
    /// Returns the HTML content as a string.
    // =========================================================================
    // Event Subscription (Session-level)
    // =========================================================================

    /// Emit an event to all registered session event listeners.
    #[allow(clippy::empty_line_after_doc_comments)]
    /// Retry policy + callbacks shared by compaction and branch-summary
    /// summarization calls (match TS `_summarizationRetryCallbacks`). Uses the
    /// same `settings.retry` budget/backoff as agent-turn retries so a single
    /// transient stream drop no longer fails the whole operation.
    fn _summarization_retry_callbacks(
        &self,
        source: &str,
        reason: Option<String>,
    ) -> pi_agent_core::pi_ai_types::RetryCallbacks {
        let listeners = self.event_listeners.clone();
        let source = source.to_string();
        let reason = reason.clone();
        let listeners_scheduled = listeners.clone();
        let listeners_attempt = listeners.clone();
        let listeners_finished = listeners;
        pi_agent_core::pi_ai_types::RetryCallbacks {
            on_retry_scheduled: Some(Arc::new(
                move |attempt, max_attempts, delay_ms, error_message| {
                    let evt = AgentSessionEvent::SummarizationRetryScheduled {
                        attempt,
                        max_attempts,
                        delay_ms,
                        error_message,
                    };
                    let batch = {
                        let l = listeners_scheduled
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        l.iter().cloned().collect::<Vec<_>>()
                    };
                    for listener in batch {
                        listener(evt.clone());
                    }
                },
            )),
            on_retry_attempt_start: Some(Arc::new(move || {
                let evt = AgentSessionEvent::SummarizationRetryAttemptStart {
                    source: source.clone(),
                    reason: reason.clone(),
                };
                let batch = {
                    let l = listeners_attempt
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    l.iter().cloned().collect::<Vec<_>>()
                };
                for listener in batch {
                    listener(evt.clone());
                }
            })),
            on_retry_finished: Some(Arc::new(move |_, _, _| {
                let evt = AgentSessionEvent::SummarizationRetryFinished {};
                let batch = {
                    let l = listeners_finished
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    l.iter().cloned().collect::<Vec<_>>()
                };
                for listener in batch {
                    listener(evt.clone());
                }
            })),
        }
    }

    fn _emit(&self, event: AgentSessionEvent) {
        let batch = {
            let listeners = self.event_listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            listeners.iter().cloned().collect::<Vec<_>>()
        };
        for listener in batch {
            listener(event.clone());
        }
    }

    /// Emit a queue_update event with current steering and follow-up messages.
    fn _emit_queue_update(&self) {
        let steering = self.steering_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let follow_up = self.follow_up_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self._emit(AgentSessionEvent::QueueUpdate {
            steering: steering.clone(),
            follow_up: follow_up.clone(),
        });
    }

    /// Emit agent_settled and resolve idle waiters.
    async fn _emit_agent_settled(&self) {
        *self.is_agent_run_active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        self._emit(AgentSessionEvent::AgentSettled);
        // Fire agent_settled to extensions
        if let Some(ref registry) = self.extension_registry {
            registry.hook_runner().fire_agent_settled().await;
        }
        self.idle_notify.notify_waiters();
    }

    /// Check whether the agent should retry after an agent_end event.
    /// Matches TS `_willRetryAfterAgentEnd`: retry must be enabled and under
    /// the attempt cap, and the last assistant message (searched from the end)
    /// must be a retryable error.
    fn _will_retry_after_agent_end(&self, event: &AgentEvent) -> bool {
        if let AgentEvent::AgentEnd { messages } = event {
            let retry_settings = self
                .settings_manager
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_retry_settings();
            let retry_attempt = *self
                .retry_attempt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // TS passes this.model?.contextWindow ?? 0; a synchronous method
            // cannot await agent state, so overflow detection falls back to
            // error-message patterns only (context_window 0 disables the
            // usage-based cases, matching TS when no context window is known).
            return will_retry_after_agent_end(messages, retry_attempt, &retry_settings, 0);
        }
        false
    }

    /// Check if an error message is retryable (transient provider/transport error).
    /// Matches TS isRetryableAssistantError() logic.
    fn _is_retryable_error_message(&self, error_message: &str) -> bool {
        is_retryable_error_message(error_message)
    }

    /// Subscribe to session-level events (AgentSessionEvent).
    /// Returns an unsubscribe handle. Call `handle.unsubscribe()` to stop receiving events.
    /// Matches TS `subscribe()` which returns AgentSessionEvent.
    pub fn subscribe_session_events(
        &self,
        listener: AgentSessionEventListener,
    ) -> SessionEventUnsubscribeHandle {
        let mut listeners = self.event_listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = listeners.len();
        listeners.push(listener);
        SessionEventUnsubscribeHandle {
            listeners: self.event_listeners.clone(),
            index,
        }
    }

    pub fn export_html(&self) -> String {
        let mgr = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = mgr.get_entries();
        let session_name = mgr
            .get_session_name()
            .unwrap_or_else(|| "Session".to_string());
        let session_id = mgr.get_session_id();
        let cwd = mgr.get_cwd();

        let mut html = String::new();
        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
        html.push_str("<meta charset=\"UTF-8\">\n");
        html.push_str(
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n",
        );
        html.push_str(&format!("<title>{}</title>\n", html_escape(&session_name)));
        html.push_str("<style>\n");
        html.push_str("body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 20px; background: #fff; color: #333; }\n");
        html.push_str(".message { margin: 12px 0; padding: 12px; border-radius: 8px; }\n");
        html.push_str(".message.user { background: #f0f7ff; border-left: 3px solid #4a90d9; }\n");
        html.push_str(
            ".message.assistant { background: #f5f5f5; border-left: 3px solid #6b7280; }\n",
        );
        html.push_str(".message.tool { background: #faf5ff; border-left: 3px solid #a855f7; font-family: monospace; font-size: 13px; }\n");
        html.push_str(".message.error { background: #fef2f2; border-left: 3px solid #ef4444; }\n");
        html.push_str(".message .role { font-weight: 600; font-size: 12px; text-transform: uppercase; color: #666; margin-bottom: 4px; }\n");
        html.push_str(".message .content { white-space: pre-wrap; word-break: break-word; }\n");
        html.push_str(".message .timestamp { font-size: 11px; color: #999; margin-top: 4px; }\n");
        html.push_str(".header { text-align: center; margin-bottom: 24px; padding-bottom: 16px; border-bottom: 1px solid #e5e7eb; }\n");
        html.push_str(".header h1 { font-size: 20px; margin: 0; }\n");
        html.push_str(".header .meta { font-size: 12px; color: #666; margin-top: 4px; }\n");
        html.push_str("</style>\n</head>\n<body>\n");

        // Header
        html.push_str("<div class=\"header\">\n");
        html.push_str(&format!("<h1>{}</h1>\n", html_escape(&session_name)));
        html.push_str(&format!(
            "<div class=\"meta\">Session: {} | CWD: {}</div>\n",
            html_escape(session_id),
            html_escape(cwd)
        ));
        html.push_str("</div>\n");

        // Messages
        for entry in entries {
            match entry {
                crate::core::session_manager::SessionEntry::Message {
                    message, timestamp, ..
                } => {
                    let role = message
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let content = message
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let css_class = match role {
                        "user" => "user",
                        "assistant" => "assistant",
                        "toolResult" | "tool_result" => "tool",
                        _ => "",
                    };
                    html.push_str(&format!("<div class=\"message {}\">\n", css_class));
                    html.push_str(&format!(
                        "<div class=\"role\">{}</div>\n",
                        html_escape(role)
                    ));
                    html.push_str(&format!(
                        "<div class=\"content\">{}</div>\n",
                        html_escape(content)
                    ));
                    html.push_str(&format!(
                        "<div class=\"timestamp\">{}</div>\n",
                        html_escape(timestamp)
                    ));
                    html.push_str("</div>\n");
                }
                crate::core::session_manager::SessionEntry::Compaction {
                    summary,
                    timestamp,
                    ..
                } => {
                    html.push_str("<div class=\"message\" style=\"background: #fffbeb; border-left: 3px solid #f59e0b;\">\n");
                    html.push_str("<div class=\"role\">Compaction</div>\n");
                    html.push_str(&format!(
                        "<div class=\"content\">{}</div>\n",
                        html_escape(summary)
                    ));
                    html.push_str(&format!(
                        "<div class=\"timestamp\">{}</div>\n",
                        html_escape(timestamp)
                    ));
                    html.push_str("</div>\n");
                }
                crate::core::session_manager::SessionEntry::BranchSummary {
                    summary,
                    timestamp,
                    ..
                } => {
                    html.push_str("<div class=\"message\" style=\"background: #f0fdf4; border-left: 3px solid #22c55e;\">\n");
                    html.push_str("<div class=\"role\">Branch Summary</div>\n");
                    html.push_str(&format!(
                        "<div class=\"content\">{}</div>\n",
                        html_escape(summary)
                    ));
                    html.push_str(&format!(
                        "<div class=\"timestamp\">{}</div>\n",
                        html_escape(timestamp)
                    ));
                    html.push_str("</div>\n");
                }
                _ => {}
            }
        }

        html.push_str("</body>\n</html>\n");
        html
    }

    /// Export the session as HTML to a file, matching the original exportHTMLToFile().
    /// Returns the file path on success.
    pub fn export_html_to_file(&self, file_path: Option<&str>) -> Result<String, String> {
        let html = self.export_html();
        let path = file_path.map(|p| p.to_string()).unwrap_or_else(|| {
            let mgr = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let session_id = mgr.get_session_id();
            format!("session_{}.html", session_id)
        });
        std::fs::write(&path, &html).map_err(|e| format!("Failed to write HTML file: {}", e))?;
        Ok(path)
    }

    /// Export the current session branch to a JSONL file, matching TS exportToJsonl().
    /// Writes the session header followed by all entries on the current branch path.
    pub fn export_to_jsonl(&self, output_path: Option<&str>) -> Result<String, String> {
        use crate::core::session_manager::CURRENT_SESSION_VERSION;
        use crate::utils::paths::{resolve_path, PathOptions};
        let mgr = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let cwd = mgr.get_cwd().to_string();
        let raw_path = output_path.map(|p| p.to_string()).unwrap_or_else(|| {
            let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S");
            format!("session-{}.jsonl", ts)
        });
        let file_path = resolve_path(&raw_path, &cwd, &PathOptions::default());

        let dir = std::path::Path::new(&file_path)
            .parent()
            .ok_or_else(|| format!("Invalid output path: {file_path} has no parent directory"))?;
        if !dir.exists() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }

        let header = serde_json::json!({
            "type": "session",
            "version": CURRENT_SESSION_VERSION,
            "id": mgr.get_session_id(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "cwd": mgr.get_cwd(),
        });

        let branch_entries = mgr.get_branch(None);
        let mut lines = vec![serde_json::to_string(&header).map_err(|e| e.to_string())?];

        // Re-chain parentIds to form a linear sequence
        let mut prev_id: Option<String> = None;
        for entry in &branch_entries {
            let mut linear = serde_json::to_value(entry).map_err(|e| e.to_string())?;
            if let Some(obj) = linear.as_object_mut() {
                if let Some(prev) = &prev_id {
                    obj.insert(
                        "parentId".to_string(),
                        serde_json::Value::String(prev.clone()),
                    );
                } else {
                    obj.insert("parentId".to_string(), serde_json::Value::Null);
                }
            }
            lines.push(serde_json::to_string(&linear).map_err(|e| e.to_string())?);
            prev_id = Some(entry.id().to_string());
        }

        std::fs::write(
            &file_path,
            lines.join(
                "
",
            ) + "
",
        )
        .map_err(|e| e.to_string())?;
        Ok(file_path)
    }

    /// Get all user messages from session for fork selector, matching TS getUserMessagesForForking().
    pub fn get_user_messages_for_forking(&self) -> Vec<(String, String)> {
        let mgr = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = mgr.get_entries();
        let mut result = Vec::new();

        for entry in &entries {
            if let SessionEntry::Message { message, .. } = entry {
                if let Some(role) = message.get("role").and_then(|v| v.as_str()) {
                    if role == "user" {
                        // Extract text from string or content-block array
                        // (matching TS contentText()).
                        let text = message_content_text(message);
                        if !text.is_empty() {
                            result.push((entry.id().to_string(), text));
                        }
                    }
                }
            }
        }

        result
    }
}

/// Extract plain text from a message's `content` field, which may be a
/// string or an array of content blocks (matching TS `contentText`).
fn message_content_text(message: &serde_json::Value) -> String {
    match message.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    b.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<&str>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Escape HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Consume a stream_fn event stream into a final `AssistantMessage`.
/// Errors and premature stream endings are normalized to an error message so
/// the retry loop can classify them (match TS `streamFn(...).result()`).
async fn consume_stream_to_message(
    stream_fn: pi_agent_core::types::StreamFn,
    model: pi_agent_core::pi_ai_types::Model,
    context: pi_agent_core::pi_ai_types::Context,
    fn_options: pi_agent_core::types::StreamFnOptions,
) -> pi_agent_core::pi_ai_types::AssistantMessage {
    let error_message = |e: String| pi_agent_core::pi_ai_types::AssistantMessage {
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: pi_agent_core::pi_ai_types::Usage::default(),
        stop_reason: pi_agent_core::pi_ai_types::StopReason::Error,
        error_message: Some(e),
        raw_stop_reason: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    };

    match stream_fn(model.clone(), context, None, fn_options).await {
        Ok(mut stream) => {
            use futures::StreamExt;
            let mut last_message: Option<pi_agent_core::pi_ai_types::AssistantMessage> = None;
            while let Some(event) = stream.next().await {
                if let pi_agent_core::pi_ai_types::AssistantMessageEvent::Done { message, .. } =
                    &event
                {
                    last_message = Some(message.clone());
                    break;
                }
            }
            last_message.unwrap_or_else(|| {
                error_message("Stream ended without a terminal event".to_string())
            })
        }
        Err(e) => error_message(e.to_string()),
    }
}

impl AgentSession {

    // =========================================================================
    // Extension Message Handling
    // =========================================================================

    /// Send a user message (for extensions), matching original sendUserMessage().
    /// Send a user message, matching TS sendUserMessage().
    /// Supports `deliverAs` option for streaming behavior.
    ///
    /// Propagates `prompt()` errors (no model / no API key) so extension
    /// callers can surface them, matching TS `sendUserMessage()` which lets
    /// `prompt()` throw and the caller catches it.
    pub async fn send_user_message(
        &self,
        content: &str,
        options: Option<SendUserMessageOptions>,
    ) -> Result<(), String> {
        let opts = options.unwrap_or_default();
        self.prompt(
            content,
            Some(PromptOptions {
                expand_prompt_templates: Some(false),
                images: None,
                streaming_behavior: opts.deliver_as,
                source: Some("extension".to_string()),
                preflight_result: None,
            }),
        )
        .await
    }

    // =========================================================================
    // Lifecycle
    // =========================================================================

    /// Invalidate the extension context, marking it as stale.
    /// Called by AgentSessionRuntime during session replacement.
    pub fn invalidate_ext_ctx(&self) {
        // self.ext_ctx.invalidate() removed — ExtensionContext no longer has this method
    }

    /// Execute a bash command directly, matching the original executeBash().
    /// Returns the command output as a string.
    /// Execute a bash command, matching TS executeBash().
    /// Supports abort via bash_abort controller and optional on_chunk callback.
    pub async fn execute_bash(
        &self,
        command: &str,
        on_chunk: Option<ChunkCallback>,
        exclude_from_context: Option<bool>,
        id: Option<String>,
    ) -> Result<crate::core::bash_executor::BashExecutorResult, String> {
        use crate::core::bash_executor::{BashExecutor, BashExecutorOptions};

        // Dispatch user_bash event to extensions
        // If an extension handles the command, return its result
        if let Some(ref registry) = self.extension_registry {
            if let Some(result) = crate::core::extensions::dispatcher::dispatch_user_bash(
                registry,
                command,
                &self.cwd,
                &self.ext_ctx,
            )
            .await
            {
                // Extension handled the command; return its result
                if let Some(ref result_val) = result.result {
                    if let Ok(parsed) = serde_json::from_value(result_val.clone()) {
                        return Ok(parsed);
                    }
                }
            }
        }

        // Create abort signal, matching TS `this._bashAbortController = new AbortController()`
        let (tx, rx) = tokio::sync::watch::channel(false);
        *self.bash_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);

        let executor = BashExecutor::new(&self.cwd);
        // Apply the configured shell path and command prefix (matching TS
        // executeBash() which reads settingsManager.getShellPath() and
        // getShellCommandPrefix()).
        let (shell_path, command_prefix) = {
            let sm = self
                .settings_manager
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                sm.get_shell_path().map(|s| s.to_string()),
                sm.get_shell_command_prefix().map(|s| s.to_string()),
            )
        };
        // Forward output deltas to the caller AND emit bash_execution_update
        // events (match TS #6971).
        let event_listeners = self.event_listeners.clone();
        let on_chunk = {
            let on_chunk = on_chunk;
            let id = id.clone();
            Box::new(move |delta: &str| {
                if let Some(cb) = &on_chunk {
                    cb(delta);
                }
                let evt = AgentSessionEvent::BashExecutionUpdate {
                    id: id.clone(),
                    delta: delta.to_string(),
                };
                let batch = {
                    let l = event_listeners
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    l.iter().cloned().collect::<Vec<_>>()
                };
                for listener in batch {
                    listener(evt.clone());
                }
            })
        };
        let options = BashExecutorOptions {
            on_chunk: Some(on_chunk),
            signal: Some(rx),
            shell_path,
            command_prefix,
        };

        let result = executor
            .execute(command, Some(options))
            .await
            .map_err(|e| e.to_string())?;

        // Clear abort controller, matching TS `this._bashAbortController = undefined`
        *self.bash_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;

        // Record bash result in session history, matching TS recordBashResult()
        self.record_bash_result(
            command,
            &result.output,
            result.exit_code,
            result.cancelled,
            result.truncated,
            result.full_output_path.clone(),
            exclude_from_context,
        )
        .await;

        Ok(result)
    }

    /// Whether a bash command is currently running, matching TS `get isBashRunning()`.
    pub fn is_bash_running(&self) -> bool {
        self.bash_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// Record a bash execution result, matching TS `recordBashResult()`.
    /// Queues the result when the agent is streaming; appends immediately otherwise.
    pub async fn record_bash_result(
        &self,
        command: &str,
        output: &str,
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
        exclude_from_context: Option<bool>,
    ) {
        use pi_agent_core::types::AgentMessage;

        let bash_message = AgentMessage::BashExecution {
            command: command.to_string(),
            output: output.to_string(),
            exit_code,
            cancelled,
            truncated,
            full_output_path,
            timestamp: chrono::Utc::now().timestamp_millis(),
            exclude_from_context,
        };

        // If agent is streaming, queue for later (matching TS _pendingBashMessages)
        if *self.is_agent_run_active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            let value = serde_json::to_value(&bash_message).unwrap_or_default();
            self.pending_bash_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(value);
        } else {
            // Add to agent state immediately
            // Write through the shared state — `state()` returns a clone.
            self.agent
                .update_state(|s| s.messages.push(bash_message))
                .await;
            // Note: session persistence is handled by the agent event handler
        }
    }

    /// Whether there are pending bash messages waiting to be flushed,
    /// matching TS `get hasPendingBashMessages()`.
    pub fn has_pending_bash_messages(&self) -> bool {
        !self.pending_bash_messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty()
    }

    /// Abort running bash command, matching TS `abortBash()`.
    pub fn abort_bash(&self) {
        if let Some(sender) = self.bash_abort.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            let _ = sender.send(true);
        }
    }

    /// Abort the current operation and wait for the agent to become idle,
    /// matching TS `abort()` which calls abortRetry() + agent.abort() +
    /// waitForIdle().
    pub async fn abort(&self) {
        self.abort_retry();
        self.agent.abort().await;
        self.wait_for_idle().await;
    }

    /// Wait for the agent to finish processing (idle).
    pub async fn wait_for_idle(&self) {
        if !*self.is_agent_run_active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            return;
        }
        self.idle_notify.notified().await;
    }

    /// Set steering message mode, matching TS setSteeringMode().
    /// Saves to settings and updates the agent's queue mode.
    pub async fn set_steering_mode(&self, mode: QueueMode) {
        self.agent.set_steering_mode(mode).await;
        if let Ok(mut sm) = self.settings_manager.lock() {
            let mode_str = match mode {
                QueueMode::All => "all",
                QueueMode::OneAtATime => "one-at-a-time",
            };
            let mode = if mode_str == "all" {
                crate::core::settings_manager::SteeringMode::All
            } else {
                crate::core::settings_manager::SteeringMode::OneAtATime
            };
            sm.set_steering_mode(mode);
        }
    }

    /// Set follow-up message mode, matching TS setFollowUpMode().
    /// Saves to settings and updates the agent's queue mode.
    pub async fn set_follow_up_mode(&self, mode: QueueMode) {
        self.agent.set_follow_up_mode(mode).await;
        if let Ok(mut sm) = self.settings_manager.lock() {
            let mode_str = match mode {
                QueueMode::All => "all",
                QueueMode::OneAtATime => "one-at-a-time",
            };
            let mode = if mode_str == "all" {
                crate::core::settings_manager::FollowUpMode::All
            } else {
                crate::core::settings_manager::FollowUpMode::OneAtATime
            };
            sm.set_follow_up_mode(mode);
        }
    }

    /// Set auto-compaction enabled, matching TS setAutoCompactionEnabled().
    pub fn set_auto_compaction_enabled(&self, enabled: bool) {
        if let Ok(mut sm) = self.settings_manager.lock() {
            sm.set_compaction_enabled(enabled);
        }
    }

    /// Whether auto-compaction is enabled, matching TS `get autoCompactionEnabled()`.
    pub fn auto_compaction_enabled(&self) -> bool {
        self.settings_manager
            .lock()
            .map(|sm| sm.get_compaction_enabled())
            .unwrap_or(true)
    }

    /// Get a reference to the loaded resources (skills, prompt templates, context files),
    /// matching TS `get resourceLoader()`.
    pub fn resource_loader(&self) -> Option<&LoadedResources> {
        self.resources.as_ref()
    }

    /// Reload session state from disk, matching TS reload().
    /// Refreshes settings, queue modes, and resources.
    pub async fn reload(&self) {
        // Reload settings
        if let Ok(mut sm) = self.settings_manager.lock() {
            sm.reload();
        }
        // Sync queue modes from settings (matching TS syncQueueModesFromSettings)
        self._sync_queue_modes_from_settings().await;
        // Reload resources
        // Note: resources are loaded once at construction time in the current
        // implementation. Full hot-reload of resources would require re-reading
        // from disk, which is a future enhancement.
    }

    /// Sync queue modes from settings, matching TS syncQueueModesFromSettings().
    async fn _sync_queue_modes_from_settings(&self) {
        let modes = if let Ok(sm) = self.settings_manager.lock() {
            Some((sm.get_steering_mode(), sm.get_follow_up_mode()))
        } else {
            None
        };
        if let Some((steering, follow_up)) = modes {
            let steering_mode = match steering {
                crate::core::settings_manager::SteeringMode::All => {
                    pi_agent_core::types::QueueMode::All
                }
                crate::core::settings_manager::SteeringMode::OneAtATime => {
                    pi_agent_core::types::QueueMode::OneAtATime
                }
            };
            let follow_up_mode = match follow_up {
                crate::core::settings_manager::FollowUpMode::All => {
                    pi_agent_core::types::QueueMode::All
                }
                crate::core::settings_manager::FollowUpMode::OneAtATime => {
                    pi_agent_core::types::QueueMode::OneAtATime
                }
            };
            self.agent.set_steering_mode(steering_mode).await;
            self.agent.set_follow_up_mode(follow_up_mode).await;
        }
    }

    pub fn subscribe(&self, listener: AgentSessionEventListener) -> SessionEventUnsubscribeHandle {
        let mut listeners = self.event_listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = listeners.len();
        listeners.push(listener);
        SessionEventUnsubscribeHandle {
            listeners: self.event_listeners.clone(),
            index,
        }
    }


    /// Internal cleanup: abort in-flight operations, disconnect from agent,
    /// and clear event listeners.
    ///
    /// This does NOT dispatch session_shutdown — that is the responsibility
    /// of AgentSessionRuntime::teardown_current(). Callers that use AgentSession
    /// directly (without AgentSessionRuntime) must dispatch session_shutdown
    /// themselves before calling this method.
    pub async fn dispose_inner(&mut self) {
        // Abort all in-flight operations
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.abort_retry();
            self.abort_compaction();
            self.abort_branch_summary();
            self.abort_bash();
        }));
        // agent.abort() is async, so we can't use catch_unwind
        self.agent.abort().await;
        // Disconnect from agent
        if let Some(handle) = self._agent_subscription.take() {
            handle.unsubscribe().await;
        }
        // Clear event listeners
        self.event_listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }

    /// Replace the session manager and reload messages from the new session.
    /// This is a low-level operation; for full lifecycle management with
    /// extension events, use AgentSessionRuntime instead.
    pub fn replace_session_manager(&mut self, new_mgr: SessionManager) {
        *self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = new_mgr;
    }

    /// Create a new session (session manager level), matching the original
    /// simplified new_session(). For full lifecycle management with extension
    /// events and factory-based creation, use AgentSessionRuntime::new_session().
    pub async fn session_mgr_new(&mut self, parent_session: Option<&str>) {
        use crate::core::session_manager::SessionManager as SM;
        let session_dir = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_session_dir().to_string_lossy().to_string();
        let new_session_opts = parent_session.map(|p| {
            crate::core::session_manager::NewSessionOptions {
                id: None,
                parent_session: Some(p.to_string()),
            }
        });
        let new_mgr = SM::new(&self.cwd, &session_dir, None, true, new_session_opts);
        *self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = new_mgr;
    }

    /// Switch to a different session file (session manager level), matching
    /// the original simplified switch_session(). For full lifecycle management
    /// with extension events and factory-based creation, use AgentSessionRuntime::switch_session().
    pub async fn session_mgr_switch(&mut self, session_path: &str, cwd_override: Option<&str>) -> Result<(), String> {
        use crate::core::session_manager::SessionManager as SM;
        let path = std::path::Path::new(session_path);
        if !path.exists() {
            return Err(format!("Session file not found: {}", session_path));
        }
        if !crate::core::session_manager::is_valid_session_file(path) {
            return Err(format!("Invalid session file: {}", session_path));
        }
        let session_dir = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_session_dir().to_string_lossy().to_string();
        let effective_cwd = cwd_override.unwrap_or(&self.cwd);
        let new_mgr = SM::new(effective_cwd, &session_dir, Some(session_path), true, None);
        *self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = new_mgr;
        self.load_messages_from_session().await;
        // The session changed: invalidate the JS extension runtime so stale
        // contexts are detected (assertActive / runtime.invalidate).
        if let Some(ref invalidate) = self.js_invalidator {
            invalidate();
        }
        Ok(())
    }

    /// Fork the session at a specific entry (session manager level), matching
    /// the original simplified fork_session(). For full lifecycle management
    /// with extension events and factory-based creation, use AgentSessionRuntime::fork().
    pub async fn session_mgr_fork(&mut self, entry_id: &str) -> Result<String, String> {
        let branch_path = self.session_manager.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .create_branched_session(entry_id, None)?;
        self.session_mgr_switch(&branch_path, None).await?;
        Ok(branch_path)
    }
}

/// Whether an assistant message is a retryable error, matching TS
/// `_isRetryableError`: context overflow is handled by compaction, not retry
/// (full `isContextOverflow` detection: error patterns + silent usage
/// overflow + length-stop overflow).
fn is_retryable_error(message: &AgentMessage, context_window: u64) -> bool {
    if let AgentMessage::Assistant {
        content,
        api,
        provider,
        model,
        usage,
        stop_reason,
        error_message,
        timestamp,
    } = message
    {
        if stop_reason != &Some(pi_agent_core::pi_ai_types::StopReason::Error) {
            return false;
        }
        if let Some(ref err_msg) = error_message {
            // Context overflow is handled by compaction, not retry. A zero
            // context window disables the usage-based overflow cases
            // (matching TS `this.model?.contextWindow ?? 0`).
            let cw = if context_window > 0 {
                Some(context_window)
            } else {
                None
            };
            let msg = pi_agent_core::pi_ai_types::AssistantMessage {
                content: content.clone(),
                api: api.clone(),
                provider: provider.clone(),
                model: model.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: usage.clone(),
                stop_reason: stop_reason.clone().unwrap_or(
                    pi_agent_core::pi_ai_types::StopReason::Stop,
                ),
                error_message: error_message.clone(),
                raw_stop_reason: None,
                timestamp: *timestamp,
            };
            if pi_agent_core::pi_ai::utils::overflow::is_context_overflow(&msg, cw) {
                return false;
            }
            return is_retryable_error_message(err_msg);
        }
    }
    false
}

/// Whether the agent will retry after an agent_end, matching TS
/// `_willRetryAfterAgentEnd`: retry must be enabled and under the attempt
/// cap, and the last assistant message (searched from the end) must be a
/// retryable error.
fn will_retry_after_agent_end(
    messages: &[AgentMessage],
    retry_attempt: u32,
    retry_settings: &crate::core::settings_manager::RetrySettings,
    context_window: u64,
) -> bool {
    if !retry_settings.enabled.unwrap_or(true)
        || retry_attempt >= retry_settings.max_retries.unwrap_or(3)
    {
        return false;
    }
    for message in messages.iter().rev() {
        if let AgentMessage::Assistant { .. } = message {
            return is_retryable_error(message, context_window);
        }
    }
    false
}

/// Whether an error message matches a retryable transient provider/transport
/// error (quota/billing errors are not retryable). Shared by the agent_end
/// will_retry computation and `_is_retryable_error`.
fn is_retryable_error_message(error_message: &str) -> bool {
    // Non-retryable patterns (quota/billing/limit errors)
    let non_retryable_patterns = [
        "insufficient_quota",
        "out of budget",
        "quota exceeded",
        "billing",
        "GoUsageLimitError",
        "FreeUsageLimitError",
        "Monthly usage limit reached",
        "available balance",
    ];
    for pattern in &non_retryable_patterns {
        if error_message.to_lowercase().contains(pattern) {
            return false;
        }
    }

    // Retryable patterns (transient provider/transport errors)
    let retryable_patterns = [
        "overloaded",
        "rate limit",
        "too many requests",
        "429",
        "500",
        "502",
        "503",
        "504",
        "524",
        "service unavailable",
        "server error",
        "internal error",
        "provider returned error",
        "network error",
        "connection error",
        "connection refused",
        "fetch failed",
        "upstream connect",
        "reset before headers",
        "socket hang up",
        "timed out",
        "timeout",
        "terminated",
        "websocket closed",
        "websocket error",
        "getaddrinfo",
        "enotfound",
        "eai_again",
        "ended without",
        "stream ended before message_stop",
        "stream ended before a terminal response event",
        "http2 request did not get a response",
        "retry delay",
        "you can retry your request",
        "try your request again",
        "please retry your request",
        "ResourceExhausted",
    ];
    for pattern in &retryable_patterns {
        if error_message.to_lowercase().contains(pattern) {
            return true;
        }
    }
    false
}

/// Accumulate token/cost totals from a usage JSON value (matching TS
/// `addUsageToTotals`).
fn add_usage_to_totals(tokens: &mut TokenUsage, cost: &mut f64, usage: &serde_json::Value) {
    let obj = usage.as_object();
    let get = |key: &str| {
        obj.and_then(|o| o.get(key))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    };
    tokens.input += get("input");
    tokens.output += get("output");
    tokens.cache_read += get("cacheRead");
    tokens.cache_write += get("cacheWrite");
    if let Some(cost_total) = obj
        .and_then(|o| o.get("cost"))
        .and_then(|c| c.get("total"))
        .and_then(|v| v.as_f64())
    {
        *cost += cost_total;
    }
}

/// Compute session statistics from session entries, matching TS
/// `getSessionStats()`: message counts, tool-call count (content block type
/// "toolCall"), and token/cost totals accumulated from assistant, toolResult,
/// compaction and branch-summary usage.
fn compute_session_stats(
    entries: Vec<&crate::core::session_manager::SessionEntry>,
) -> (usize, usize, usize, usize, usize, TokenUsage, f64) {
    let mut user_messages = 0;
    let mut assistant_messages = 0;
    let mut tool_calls = 0;
    let mut tool_results = 0;
    let mut total_messages = 0;
    let mut tokens = TokenUsage::default();
    let mut cost = 0.0;

    for entry in entries {
        match entry {
            // Compaction / branch-summary entries contribute their usage.
            crate::core::session_manager::SessionEntry::Compaction { usage, .. }
            | crate::core::session_manager::SessionEntry::BranchSummary { usage, .. } => {
                if let Some(usage) = usage {
                    add_usage_to_totals(&mut tokens, &mut cost, usage);
                }
            }
            crate::core::session_manager::SessionEntry::Message { message, .. } => {
                total_messages += 1;
                if let Some(role) = message.get("role").and_then(|v| v.as_str()) {
                    match role {
                        "user" => user_messages += 1,
                        "assistant" => {
                            assistant_messages += 1;
                            // Count tool calls within assistant messages
                            // (content block type is "toolCall", matching
                            // TS which checks c.type === "toolCall").
                            if let Some(content) = message.get("content") {
                                if let Some(blocks) = content.as_array() {
                                    for block in blocks {
                                        if block.get("type").and_then(|v| v.as_str())
                                            == Some("toolCall")
                                        {
                                            tool_calls += 1;
                                        }
                                    }
                                }
                            }
                            if let Some(usage) = message.get("usage") {
                                add_usage_to_totals(&mut tokens, &mut cost, usage);
                            }
                        }
                        "toolResult" => {
                            tool_results += 1;
                            if let Some(usage) = message.get("usage") {
                                add_usage_to_totals(&mut tokens, &mut cost, usage);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    tokens.total = tokens.input + tokens.output + tokens.cache_read + tokens.cache_write;
    (
        user_messages,
        assistant_messages,
        tool_calls,
        tool_results,
        total_messages,
        tokens,
        cost,
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::core::session_manager::SessionEntry;

    fn message_entry(role: &str, content: serde_json::Value, usage: Option<serde_json::Value>) -> SessionEntry {
        let mut msg = serde_json::json!({ "role": role, "content": content });
        if let Some(u) = usage {
            msg["usage"] = u;
        }
        SessionEntry::Message {
            id: "id".into(),
            parent_id: None,
            timestamp: "0".into(),
            message: msg,
        }
    }

    /// Tool calls are counted from content blocks with type "toolCall"
    /// (matching TS `c.type === "toolCall"`), not "tool_use".
    #[test]
    fn session_stats_counts_tool_calls_from_toolcall_blocks() {
        let entries = [
            message_entry(
                "assistant",
                serde_json::json!([
                    { "type": "text", "text": "hi" },
                    { "type": "toolCall", "id": "c1", "name": "bash", "arguments": {} },
                    { "type": "toolCall", "id": "c2", "name": "read", "arguments": {} }
                ]),
                None,
            ),
            message_entry("toolResult", serde_json::json!([]), None),
            message_entry("user", serde_json::json!([{ "type": "text", "text": "q" }]), None),
        ];
        let (user, assistant, tool_calls, tool_results, total, _, _) =
            compute_session_stats(entries.iter().collect());
        assert_eq!(user, 1);
        assert_eq!(assistant, 1);
        assert_eq!(tool_calls, 2);
        assert_eq!(tool_results, 1);
        // total_messages counts message entries only (not tool calls).
        assert_eq!(total, 3);
    }

    /// Usage from assistant, toolResult, compaction and branch-summary entries
    /// is accumulated into token/cost totals (matching TS addUsageToTotals).
    #[test]
    fn session_stats_accumulates_usage_and_cost() {
        let usage = |input: u64, output: u64, cost: f64| {
            serde_json::json!({
                "input": input, "output": output, "cacheRead": 0, "cacheWrite": 0,
                "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": cost }
            })
        };
        let entries = [
            message_entry("assistant", serde_json::json!([]), Some(usage(100, 50, 0.5))),
            message_entry("toolResult", serde_json::json!([]), Some(usage(10, 0, 0.1))),
            SessionEntry::Compaction {
                id: "c".into(),
                parent_id: None,
                timestamp: "1".into(),
                summary: "s".into(),
                first_kept_entry_id: "x".into(),
                tokens_before: 0,
                details: None,
                usage: Some(usage(200, 100, 1.0)),
                from_hook: None,
            },
        ];
        let (_, _, _, _, _, tokens, cost) = compute_session_stats(entries.iter().collect());
        assert_eq!(tokens.input, 310);
        assert_eq!(tokens.output, 150);
        assert_eq!(tokens.total, 460);
        assert!((cost - 1.6).abs() < 1e-9);
    }

    /// Quota/billing errors are not retryable; transient errors are.
    #[test]
    fn retryable_error_message_classification() {
        assert!(is_retryable_error_message("529 overloaded_error: Overloaded"));
        assert!(is_retryable_error_message("network error: connection refused"));
        assert!(!is_retryable_error_message("insufficient_quota: out of credits"));
        assert!(!is_retryable_error_message("billing issue on account"));
        assert!(!is_retryable_error_message("Monthly usage limit reached"));
    }
}
