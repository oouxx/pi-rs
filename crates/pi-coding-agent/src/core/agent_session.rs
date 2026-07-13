use std::sync::Arc;

use pi_agent_core::agent::Agent;
use pi_agent_core::pi_ai_types::{ContentBlock, Model, ThinkingLevel};
use pi_agent_core::types::{
    AfterToolCallFn, AgentEvent, AgentMessage, AgentState, BeforeToolCallFn, ConvertToLlmFn,
    StreamFn, TransformContextFn,
};

use crate::core::compaction::CompactionSettings;
use crate::core::context_usage::ContextUsage;
use crate::core::event_bus::EventBusController;
use crate::core::messages;
use crate::core::model_registry::ModelRegistry;
use crate::core::resource_loader::LoadedResources;
use crate::core::session_manager::SessionManager;
use crate::core::system_prompt::{self, BuildSystemPromptOptions, ContextFile, SkillInfo};
use crate::core::extensions::{ExtensionRuntime, ToolInfoSerde};
use crate::core::tools;

// ============================================================================
// Types
// ============================================================================

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
    /// Extension tools loaded via the embedded runtime.
    pub extension_tools: Vec<ToolInfoSerde>,
    /// Embedded extension runtime for calling extension tool handlers.
    pub extension_runtime: Option<std::sync::Arc<ExtensionRuntime>>,
    /// Loaded resources (skills, extensions, prompt templates).
    pub resources: Option<LoadedResources>,
}

/// Options for AgentSession.prompt().
/// Matches the original TypeScript PromptOptions interface.
pub struct PromptOptions {
    pub images: Option<Vec<ContentBlock>>,
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
// AgentSession
// ============================================================================

pub struct AgentSession {
    agent: Agent,
    session_manager: Arc<std::sync::Mutex<SessionManager>>,
    event_bus: EventBusController,
    model_registry: ModelRegistry,
    compaction_settings: CompactionSettings,
    cwd: String,
    scoped_models: Vec<(Model, Option<ThinkingLevel>)>,
    initial_active_tool_names: Vec<String>,
    allowed_tool_names: Option<Vec<String>>,
    excluded_tool_names: Option<Vec<String>>,
    /// Subscription handles kept alive so listeners are not dropped.
    _subscriptions: Vec<pi_agent_core::agent::UnsubscribeHandle>,
    /// Embedded extension runtime handle, retained so dispose() can stop the
    /// V8 thread gracefully. Tool closures hold their own Arc clones too.
    extension_runtime: Option<Arc<ExtensionRuntime>>,
}

impl AgentSession {
    pub async fn new(
        session_manager: SessionManager,
        event_bus: EventBusController,
        model_registry: ModelRegistry,
        options: AgentSessionConfig,
    ) -> Self {
        let system_prompt = system_prompt::build_system_prompt(&BuildSystemPromptOptions {
            cwd: options.cwd.clone(),
            custom_prompt: options.custom_prompt,
            append_system_prompt: options.append_system_prompt,
            selected_tools: options.selected_tools.clone(),
            tool_snippets: options.tool_snippets,
            prompt_guidelines: options.prompt_guidelines,
            context_files: Some(options.context_files),
            skills: Some(options.skills),
        });

        let tools_options = tools::ToolsOptions::default();
        let mut tool_list = tools::create_coding_tools(&options.cwd, Some(&tools_options));

        // Merge extension tools if extension runtime is available
        if !options.extension_tools.is_empty() {
            if let Some(ref rt) = options.extension_runtime {
                let ext_tools = crate::core::extensions::create_extension_agent_tools(
                    &options.extension_tools,
                    std::sync::Arc::clone(rt),
                    options.cwd.clone(),
                );
                tool_list.extend(ext_tools);
            }
        }

        let tools: Vec<Arc<pi_agent_core::types::DynTool>> = tool_list
            .into_iter()
            .map(|t| Arc::new(t) as Arc<pi_agent_core::types::DynTool>)
            .collect();

        let initial_state = AgentState {
            system_prompt,
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
        // execution loop. When an extension runtime is present, each tool call
        // is dispatched to JS handlers that may block it (before) or transform
        // its result (after); absent a runtime, the hooks are None (no overhead).
        let (before_tool_call, after_tool_call) = match &options.extension_runtime {
            Some(rt) => {
                let before_rt = Arc::clone(rt);
                let after_rt = Arc::clone(rt);
                let before: BeforeToolCallFn = Arc::new(move |ctx, _signal| {
                    let rt = Arc::clone(&before_rt);
                    Box::pin(async move {
                        crate::core::extensions::dispatcher::dispatch_tool_call(&rt, &ctx).await
                    })
                });
                let after: AfterToolCallFn = Arc::new(move |ctx, _signal| {
                    let rt = Arc::clone(&after_rt);
                    Box::pin(async move {
                        crate::core::extensions::dispatcher::dispatch_tool_result(&rt, &ctx).await
                    })
                });
                (Some(before), Some(after))
            }
            None => (None, None),
        };

        // Wire the context event hook: extensions can modify messages before
        // they are sent to the LLM. The JS side chains handlers serially.
        let transform_context: Option<TransformContextFn> = options.extension_runtime.as_ref().map(|rt| {
            let ctx_rt = Arc::clone(rt);
            let closure = move |messages: Vec<AgentMessage>, _signal: Option<tokio::sync::watch::Receiver<bool>>| {
                let rt = Arc::clone(&ctx_rt);
                Box::pin(async move {
                    crate::core::extensions::dispatcher::dispatch_context(&rt, messages).await
                }) as std::pin::Pin<Box<dyn std::future::Future<Output = Vec<AgentMessage>> + Send>>
            };
            Arc::new(closure) as TransformContextFn
        });

        // Wire the before_provider_request event: extensions can inspect/modify
        // the provider request payload before it is sent. The on_payload callback
        // fires right before the HTTP request; we dispatch the event here so
        // extensions see the payload. (Full result-returning modification of the
        // actual request body requires a future pi-agent-core hook.)
        let on_payload: Option<Arc<dyn Fn(serde_json::Value) + Send + Sync>> =
            options.extension_runtime.as_ref().map(|rt| {
                let payload_rt = Arc::clone(rt);
                let closure = move |payload: serde_json::Value| {
                    let rt = Arc::clone(&payload_rt);
                    tokio::spawn(async move {
                        let _ = crate::core::extensions::dispatcher::dispatch_before_provider_request(
                            &rt, payload,
                        )
                        .await;
                    });
                };
                Arc::new(closure) as Arc<dyn Fn(serde_json::Value) + Send + Sync>
            });

        let agent_options = pi_agent_core::agent::AgentOptions {
            initial_state: Some(initial_state),
            convert_to_llm: Some(convert_to_llm),
            stream_fn: Some(stream_fn),
            session_id: Some(session_manager.get_session_id().to_string()),
            before_tool_call,
            after_tool_call,
            transform_context,
            on_payload,
            ..Default::default()
        };

        let agent = Agent::new(agent_options);
        let session_manager = Arc::new(std::sync::Mutex::new(session_manager));

        let initial_active_tool_names = options.initial_active_tool_names.unwrap_or_else(|| {
            vec!["read", "bash", "edit", "write"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        });

        let mut session = Self {
            agent,
            session_manager: session_manager.clone(),
            event_bus,
            model_registry,
            compaction_settings: CompactionSettings::default(),
            cwd: options.cwd,
            scoped_models: Vec::new(),
            initial_active_tool_names,
            allowed_tool_names: options.allowed_tool_names,
            excluded_tool_names: options.excluded_tool_names,
            _subscriptions: Vec::new(),
            extension_runtime: options.extension_runtime,
        };

        // Register event-driven persistence subscriber, matching the original
        // TS behavior: persist user / assistant / toolResult messages on each
        // message_end event.
        let persist_sm = session_manager.clone();
        let persist_listener: Arc<
            dyn Fn(
                    AgentEvent,
                    Option<tokio::sync::watch::Receiver<bool>>,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                + Send
                + Sync,
        > = Arc::new(move |event: AgentEvent, _signal| {
            let sm = persist_sm.clone();
            Box::pin(async move {
                if let AgentEvent::MessageEnd { ref message } = event {
                    match message {
                        AgentMessage::User { .. }
                        | AgentMessage::Assistant { .. }
                        | AgentMessage::ToolResult { .. } => {
                            let json =
                                serde_json::to_value(message).unwrap_or(serde_json::Value::Null);
                            sm.lock().unwrap().append_message(json);
                        }
                        _ => {}
                    }
                }
            })
        });
        let handle = session.agent.subscribe(persist_listener).await;
        session._subscriptions.push(handle);

        // Fire-and-forget agent-event dispatch to extensions. Each AgentEvent
        // is mapped to an extension event name + payload; the dispatch is
        // spawned detached so a slow extension handler never blocks the agent
        // event loop (the listener future returns immediately after spawning).
        if let Some(ref rt) = session.extension_runtime {
            let ff_rt = Arc::clone(rt);
            // Capture the session cwd so fire-and-forget handlers see the right
            // ctx.cwd (the dispatcher payloads don't carry it by default, which
            // would leave ctx.cwd = "/" inside __piDispatch).
            let session_cwd = session.cwd.clone();
            let ff_listener: Arc<
                dyn Fn(
                        AgentEvent,
                        Option<tokio::sync::watch::Receiver<bool>>,
                    )
                        -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                    + Send
                    + Sync,
            > = Arc::new(move |event, _signal| {
                let rt = Arc::clone(&ff_rt);
                let cwd = session_cwd.clone();
                Box::pin(async move {
                    if let Some((event_type, mut payload)) =
                        crate::core::extensions::dispatcher::fire_and_forget_from_agent_event(&event)
                    {
                        // Inject cwd so __piDispatch builds makeContext(session_cwd).
                        if let Some(obj) = payload.as_object_mut() {
                            obj.insert("cwd".to_string(), serde_json::Value::String(cwd));
                        }
                        tokio::spawn(async move {
                            let _ = rt.dispatch_fire_and_forget(event_type, payload).await;
                        });
                    }
                })
            });
            let handle = session.agent.subscribe(ff_listener).await;
            session._subscriptions.push(handle);
        }

        session
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    pub fn get_agent(&self) -> &Agent {
        &self.agent
    }

    pub async fn get_messages(&self) -> Vec<AgentMessage> {
        self.agent.state().await.messages
    }

    /// Load messages from the session manager's file entries into
    /// the agent's in-memory state. Called after restoring from a JSONL file.
    pub async fn load_messages_from_session(&self) -> usize {
        use crate::core::session_manager::SessionEntry;

        let agent_messages = {
            let mgr = self.session_manager.lock().unwrap();
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

    pub fn get_session_id(&self) -> String {
        self.session_manager.lock().unwrap().get_session_id().to_string()
    }

    pub fn get_session_name(&self) -> Option<String> {
        self.session_manager.lock().unwrap().get_session_name()
    }

    pub fn set_session_name(&mut self, name: &str) {
        self.session_manager.lock().unwrap().append_session_info(name);
    }

    pub async fn is_streaming(&self) -> bool {
        self.agent.state().await.is_streaming
    }

    pub fn get_error_message(&self) -> Option<&str> {
        None
    }

    pub fn get_context_usage(&self) -> ContextUsage {
        ContextUsage::default()
    }

    pub fn should_compact(&self) -> bool {
        false
    }

    pub fn get_last_assistant_text(&self) -> Option<String> {
        None
    }

    pub fn get_scoped_models(&self) -> &[(Model, Option<ThinkingLevel>)] {
        &self.scoped_models
    }

    pub fn set_scoped_models(&mut self, models: Vec<(Model, Option<ThinkingLevel>)>) {
        self.scoped_models = models;
    }

    pub fn get_session_manager(&self) -> std::sync::MutexGuard<'_, SessionManager> {
        self.session_manager.lock().unwrap()
    }

    pub fn get_event_bus(&self) -> &EventBusController {
        &self.event_bus
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

    // =========================================================================
    // Session Statistics
    // =========================================================================

    /// Get session statistics, matching the original getSessionStats().
    pub fn get_session_stats(&self) -> SessionStats {
        let mgr = self.session_manager.lock().unwrap();
        let entries = mgr.get_entries();

        let mut user_messages = 0;
        let mut assistant_messages = 0;
        let mut tool_calls = 0;
        let mut tool_results = 0;

        for entry in entries {
            match entry {
                crate::core::session_manager::SessionEntry::Message { message, .. } => {
                    if let Some(role) = message.get("role").and_then(|v| v.as_str()) {
                        match role {
                            "user" => user_messages += 1,
                            "assistant" => {
                                assistant_messages += 1;
                                // Count tool calls within assistant messages
                                if let Some(content) = message.get("content") {
                                    if let Some(blocks) = content.as_array() {
                                        for block in blocks {
                                            if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                                                tool_calls += 1;
                                            }
                                        }
                                    }
                                }
                            }
                            "tool_result" => tool_results += 1,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        let total_messages = user_messages + assistant_messages + tool_calls + tool_results;

        SessionStats {
            session_file: mgr.get_session_file().map(|p| p.to_string_lossy().to_string()),
            session_id: mgr.get_session_id().to_string(),
            user_messages,
            assistant_messages,
            tool_calls,
            tool_results,
            total_messages,
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
    pub async fn prompt(&mut self, text: &str, _options: Option<PromptOptions>) {
        // Refresh session state before starting the next turn
        if let Err(e) = self.session_manager.lock().unwrap().refresh_config().await {
            eprintln!("[pi] Failed to refresh session state before next turn: {e}");
        }
        self.add_user_text(text).await;
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

        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User { content, timestamp };
        // User message is persisted by the event subscriber on MessageEnd
        self.session_manager.lock().unwrap().set_run_prompt(&text);
        self.agent.process(vec![message]).await.ok();
    }

    pub async fn add_user_text(&mut self, text: &str) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User {
            content: vec![ContentBlock::text(text)],
            timestamp,
        };
        // User message is persisted by the event subscriber on MessageEnd
        self.session_manager.lock().unwrap().set_run_prompt(text);
        self.agent.process(vec![message]).await.ok();
    }

    // =========================================================================
    // Model Management
    // =========================================================================

    /// Set the model on the agent, matching the original setModel().
    /// Dispatches `model_select` to extensions.
    pub async fn set_model(&mut self, model: Model) {
        let model_id = model.id.clone();
        let mut state = self.agent.state().await;
        let previous_model_id = state.model.id.clone();
        state.model = model;
        drop(state);
        // Dispatch model_select to extensions (fire-and-forget)
        if let Some(ref rt) = self.extension_runtime {
            crate::core::extensions::dispatcher::dispatch_model_select(
                rt,
                &model_id,
                Some(&previous_model_id),
                "set",
            )
            .await;
        }
    }

    /// Cycle through scoped models, matching the original cycleModel().
    /// Returns the new model and thinking level, and whether it's a scoped model.
    pub async fn cycle_model(&mut self, direction: &str) -> Option<(Model, Option<ThinkingLevel>, bool)> {
        if self.scoped_models.is_empty() {
            return None;
        }

        let current_model = self.agent.state().await.model;
        let current_idx = self.scoped_models.iter().position(|(m, _)| {
            m.provider == current_model.provider && m.id == current_model.id
        });

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
        Some((model, thinking_level, true))
    }

    // =========================================================================
    // Compaction
    // =========================================================================

    /// Check whether compaction should be triggered, matching the original shouldCompact().
    pub fn check_should_compact(&self, total_tokens: u64, context_window: u64) -> bool {
        use crate::core::compaction;
        compaction::should_compact(total_tokens, context_window, &self.compaction_settings)
    }

    /// Trigger compaction, matching the original compact().
    /// Returns a summary string on success.
    pub async fn compact(&self, _custom_instructions: Option<&str>) -> Result<String, String> {
        use crate::core::compaction;

        let messages = self.agent.state().await.messages;
        let total_tokens = messages.len() as u64 * 100; // rough estimate
        let context_window = 128_000;

        if !compaction::should_compact(total_tokens, context_window, &self.compaction_settings) {
            return Err("Compaction not needed".to_string());
        }

        let keep_recent_turns = 5usize;
        let cut_point = compaction::find_compaction_cut_point(&messages, keep_recent_turns);

        let _prepared = compaction::prepare_compaction(&messages, keep_recent_turns, self.compaction_settings.clone());

        // Record compaction in session manager
        let summary = format!("Compacted {} messages", messages.len());
        {
            let mut mgr = self.session_manager.lock().unwrap();
            mgr.append_compaction(&summary, &cut_point.first_kept_entry_index.to_string(), 0, None, None);
        }

        Ok(summary)
    }

    // =========================================================================
    // Custom Messages
    // =========================================================================

    /// Send a custom message (for extensions), matching the original sendCustomMessage().
    pub async fn send_custom_message(&mut self, custom_type: &str, content: &str) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User {
            content: vec![ContentBlock::text(content)],
            timestamp,
        };
        self.session_manager
            .lock()
            .unwrap()
            .append_custom_message_entry(custom_type, serde_json::to_value(&message).unwrap_or_default(), true, None);
        self.agent.process(vec![message]).await.ok();
    }

    // =========================================================================
    // Streaming Queue Management
    // =========================================================================

    /// Queue a steering message (interrupts current stream), matching original steer().
    pub async fn steer(&self, text: &str) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User {
            content: vec![ContentBlock::text(text)],
            timestamp,
        };
        self.agent.steer(message).await;
    }

    /// Queue a follow-up message (waits for current stream), matching original followUp().
    pub async fn follow_up(&self, text: &str) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User {
            content: vec![ContentBlock::text(text)],
            timestamp,
        };
        self.agent.follow_up(message).await;
    }

    /// Check if there are queued messages, matching original hasQueuedMessages().
    pub async fn has_queued_messages(&self) -> bool {
        self.agent.has_queued_messages().await
    }

    /// Clear all queued messages, matching original clearAllQueues().
    pub async fn clear_all_queues(&self) {
        self.agent.clear_all_queues().await;
    }

    // =========================================================================
    // Session Lifecycle (switch / new / fork / import)
    // =========================================================================

    /// Switch to a different session file, matching original switchSession().
    pub async fn switch_session(
        &mut self,
        session_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<(), String> {
        use crate::core::session_manager::SessionManager as SM;

        let session_dir = self
            .session_manager
            .lock()
            .unwrap()
            .get_session_dir()
            .to_string_lossy()
            .to_string();

        let effective_cwd = cwd_override.unwrap_or(&self.cwd);
        let fallback_cwd = self.cwd.clone();
        let new_mgr = SM::new(effective_cwd, &session_dir, Some(session_path), true, None);

        // Check if session cwd exists
        let session_cwd = new_mgr.get_cwd();
        let session_file_opt = new_mgr.get_session_file().map(|p| p.to_string_lossy().to_string());
        if let Some(ref sf) = session_file_opt {
            if !session_cwd.is_empty() && !std::path::Path::new(session_cwd).exists() {
                return Err(format!(
                    "Stored session working directory does not exist: {}\nSession file: {}\nCurrent working directory: {}",
                    session_cwd, sf, fallback_cwd
                ));
            }
        }

        // Replace the session manager
        *self.session_manager.lock().unwrap() = new_mgr;

        // Reload messages from the new session
        self.load_messages_from_session().await;

        Ok(())
    }

    /// Create a new session, matching original newSession().
    pub async fn new_session(&mut self, parent_session: Option<&str>) {
        use crate::core::session_manager::SessionManager as SM;

        let session_dir = self
            .session_manager
            .lock()
            .unwrap()
            .get_session_dir()
            .to_string_lossy()
            .to_string();

        let new_session_opts = parent_session.map(|p| {
            crate::core::session_manager::NewSessionOptions {
                id: None,
                parent_session: Some(p.to_string()),
            }
        });

        let new_mgr = SM::new(&self.cwd, &session_dir, None, true, new_session_opts);
        *self.session_manager.lock().unwrap() = new_mgr;
    }

    /// Fork the session at a specific entry, matching original fork().
    /// Returns the forked session path on success.
    pub async fn fork_session(&mut self, entry_id: &str) -> Result<String, String> {
        use crate::core::session_manager::SessionManager as SM;

        let session_dir = self
            .session_manager
            .lock()
            .unwrap()
            .get_session_dir()
            .to_string_lossy()
            .to_string();

        let session_file = self
            .session_manager
            .lock()
            .unwrap()
            .get_session_file()
            .map(|p| p.to_string_lossy().to_string())
            .ok_or_else(|| "Session is not persisted".to_string())?;

        // Open the current session file
        let mut mgr = SM::new(&self.cwd, &session_dir, Some(&session_file), true, None);

        // Create a new session file for the branch
        let branch_path = format!("{}.branch.jsonl", session_file);
        mgr.set_session_file(&branch_path);

        *self.session_manager.lock().unwrap() = mgr;
        Ok(branch_path)
    }

    /// Import a session from a JSONL file, matching original importFromJsonl().
    pub async fn import_from_jsonl(
        &mut self,
        input_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<(), String> {
        use crate::core::session_manager::SessionManager as SM;

        let path = std::path::Path::new(input_path);
        if !path.exists() {
            return Err(format!("File not found: {}", input_path));
        }

        let session_dir = self
            .session_manager
            .lock()
            .unwrap()
            .get_session_dir()
            .to_string_lossy()
            .to_string();

        let effective_cwd = cwd_override.unwrap_or(&self.cwd);
        let new_mgr = SM::new(effective_cwd, &session_dir, Some(input_path), true, None);

        let fallback_cwd = self.cwd.clone();
        let session_cwd = new_mgr.get_cwd();
        let session_file_opt = new_mgr.get_session_file().map(|p| p.to_string_lossy().to_string());
        if let Some(ref sf) = session_file_opt {
            if !session_cwd.is_empty() && !std::path::Path::new(session_cwd).exists() {
                return Err(format!(
                    "Stored session working directory does not exist: {}\nSession file: {}\nCurrent working directory: {}",
                    session_cwd, sf, fallback_cwd
                ));
            }
        }

        *self.session_manager.lock().unwrap() = new_mgr;
        self.load_messages_from_session().await;

        Ok(())
    }

    // =========================================================================
    // Extension Message Handling
    // =========================================================================

    /// Send a user message (for extensions), matching original sendUserMessage().
    pub async fn send_user_message(&mut self, content: &str) {
        self.add_user_text(content).await;
    }

    // =========================================================================
    // Lifecycle
    // =========================================================================

    pub async fn abort(&self) {
        self.agent.abort().await;
    }

    /// Wait for the agent to finish processing (idle).
    pub async fn wait_for_idle(&self) {
        self.agent.wait_for_idle().await;
    }

    pub async fn subscribe(
        &mut self,
        listener: Arc<
            dyn Fn(
                    AgentEvent,
                    Option<tokio::sync::watch::Receiver<bool>>,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                + Send
                + Sync,
        >,
    ) {
        let handle = self.agent.subscribe(listener).await;
        self._subscriptions.push(handle);
    }

    pub async fn dispose(self) {
        // Dispatch session_shutdown event before stopping the runtime, so
        // extensions can perform cleanup (e.g. flush state, close connections).
        if let Some(ref rt) = self.extension_runtime {
            crate::core::extensions::dispatcher::dispatch_session_shutdown(rt, "quit").await;
        }
        // Gracefully stop the embedded extension runtime so the V8 thread and
        // its isolate are torn down rather than lingering until process exit.
        if let Some(rt) = self.extension_runtime {
            let _ = rt.stop();
        }
    }
}
