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
use crate::core::extensions::{ExtensionContext, ExtensionEvent, ExtensionRegistry};
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
    /// Extension registry (Rust native extensions).
    pub extension_registry: Option<std::sync::Arc<ExtensionRegistry>>,
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
    _subscriptions: Vec<pi_agent_core::agent::UnsubscribeHandle>,
    /// Extension registry (Rust native extensions).
    extension_registry: Option<Arc<ExtensionRegistry>>,
    /// Cached extension context for dispatch calls.
    ext_ctx: ExtensionContext,
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

        // Extension tools are registered at compile time via ExtensionRegistry.
        // The registry's collect_tools() is called in sdk.rs before session creation.
        // Tool execution is handled by the built-in tool implementations.

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
        // execution loop. When an extension registry is present, each tool call
        // is dispatched to extension handlers that may block it (before) or
        // transform its result (after).
        let ext_ctx = ExtensionContext::new(
            options.cwd.clone(),
            false,
            crate::core::extensions::ExtensionUIContext {
                notify: std::sync::Arc::new(|msg, _level| eprintln!("[pi] {msg}")),
                set_status: std::sync::Arc::new(|_key, _value| {}),
                confirm: std::sync::Arc::new(|_title, _msg| false),
            },
            crate::core::extensions::RuntimeHandle::noop(),
        );
        let shared_ext_ctx = Arc::new(ext_ctx);

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
                        crate::core::extensions::dispatcher::dispatch_tool_call(&reg, &ctx, &ctx_ref).await
                    })
                });
                let after: AfterToolCallFn = Arc::new(move |ctx, _signal| {
                    let reg = Arc::clone(&after_reg);
                    let ctx_ref = Arc::clone(&after_ctx);
                    Box::pin(async move {
                        crate::core::extensions::dispatcher::dispatch_tool_result(&reg, &ctx, &ctx_ref).await
                    })
                });
                (Some(before), Some(after))
            }
            None => (None, None),
        };

        // Wire the context event hook: extensions can modify messages before
        // they are sent to the LLM.
        let transform_context: Option<TransformContextFn> = options.extension_registry.as_ref().map(|registry| {
            let dispatch_reg = Arc::clone(registry);
            let ctx_clone = Arc::clone(&shared_ext_ctx);
            let closure = move |messages: Vec<AgentMessage>, _signal: Option<tokio::sync::watch::Receiver<bool>>| {
                let reg = Arc::clone(&dispatch_reg);
                let ctx_ref = Arc::clone(&ctx_clone);
                Box::pin(async move {
                    crate::core::extensions::dispatcher::dispatch_context(&reg, messages, &ctx_ref).await
                }) as std::pin::Pin<Box<dyn std::future::Future<Output = Vec<AgentMessage>> + Send>>
            };
            Arc::new(closure) as TransformContextFn
        });

        // Wire the before_provider_request event: extensions can inspect/modify
        // the provider request payload before it is sent.
        let on_payload: Option<Arc<dyn Fn(serde_json::Value) + Send + Sync>> =
            options.extension_registry.as_ref().map(|registry| {
                let payload_reg = Arc::clone(registry);
                let ctx_clone = Arc::clone(&shared_ext_ctx);
                let closure = move |payload: serde_json::Value| {
                    let reg = Arc::clone(&payload_reg);
                    let ctx_ref = Arc::clone(&ctx_clone);
                    tokio::spawn(async move {
                        let _ = crate::core::extensions::dispatcher::dispatch_before_provider_request(
                            &reg, payload, &ctx_ref,
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

        let session_cwd = options.cwd.clone();
        let mut session = Self {
            agent,
            session_manager: session_manager.clone(),
            event_bus,
            model_registry,
            compaction_settings: CompactionSettings::default(),
            cwd: session_cwd.clone(),
            scoped_models: Vec::new(),
            initial_active_tool_names,
            allowed_tool_names: options.allowed_tool_names,
            excluded_tool_names: options.excluded_tool_names,
            _subscriptions: Vec::new(),
            extension_registry: options.extension_registry,
            ext_ctx: ExtensionContext::new(
                session_cwd,
                false,
                crate::core::extensions::ExtensionUIContext {
                    notify: std::sync::Arc::new(|msg, _level| eprintln!("[pi] {msg}")),
                    set_status: std::sync::Arc::new(|_key, _value| {}),
                    confirm: std::sync::Arc::new(|_title, _msg| false),
                },
                crate::core::extensions::RuntimeHandle::noop(),
            ),
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

        // Agent-event dispatch to extensions via ExtensionRegistry.
        // Fire-and-forget events (all except message_end) are spawned detached
        // so a slow handler never blocks the agent event loop.
        // message_end is awaited inline so the extension can process it before
        // the message is persisted (even though the result is not yet used to
        // replace the message in-place — that requires agent loop changes).
        if let Some(ref registry) = session.extension_registry {
            let dispatch_reg = Arc::clone(registry);
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
                let reg = Arc::clone(&dispatch_reg);
                let cwd = session_cwd.clone();
                Box::pin(async move {
                    if let Some(evt) =
                        crate::core::extensions::dispatcher::event_from_agent_event(&event)
                    {
                        let ext_ctx = ExtensionContext::new(
                            cwd.clone(),
                            false,
                            crate::core::extensions::ExtensionUIContext {
                                notify: std::sync::Arc::new(|msg, _level| eprintln!("[pi] {msg}")),
                                set_status: std::sync::Arc::new(|_key, _value| {}),
                                confirm: std::sync::Arc::new(|_title, _msg| false),
                            },
                            crate::core::extensions::RuntimeHandle::noop(),
                        );
                        // Await message_end inline so extensions can process it
                        // before the message is persisted. Other events are
                        // fire-and-forget to avoid blocking the agent loop.
                        if matches!(evt, ExtensionEvent::MessageEnd { .. }) {
                            reg.dispatch_event(&evt, &ext_ctx).await;
                        } else {
                            tokio::spawn(async move {
                                reg.dispatch_event(&evt, &ext_ctx).await;
                            });
                        }
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

    pub fn get_extension_registry(&self) -> Option<Arc<ExtensionRegistry>> {
        self.extension_registry.clone()
    }

    pub fn get_session_id(&self) -> String {
        self.session_manager.lock().unwrap().get_session_id().to_string()
    }

    pub fn get_session_file(&self) -> Option<std::path::PathBuf> {
        self.session_manager.lock().unwrap().get_session_file().map(|p| p.to_path_buf())
    }

    pub fn get_session_dir(&self) -> std::path::PathBuf {
        self.session_manager.lock().unwrap().get_session_dir().to_path_buf()
    }

    pub fn get_session_name(&self) -> Option<String> {
        self.session_manager.lock().unwrap().get_session_name()
    }

    pub fn set_session_name(&mut self, name: &str) {
        self.session_manager.lock().unwrap().append_session_info(name);
        // Dispatch session_info_changed to extensions
        if let Some(ref registry) = self.extension_registry {
            let reg = Arc::clone(registry);
            let name = name.to_string();
            let ctx = self.ext_ctx.clone();
            tokio::spawn(async move {
                crate::core::extensions::dispatcher::dispatch_session_info_changed(
                    &reg, Some(&name), &ctx,
                )
                .await;
            });
        }
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
        // Extension host commands are no longer needed — the old V8-based
        // ExtensionRuntime used drain_host_commands() to process pending ops
        // from the JS thread. Rust native extensions call session methods
        // directly via the ExtensionContext.
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

        // Dispatch before_agent_start to extensions before the agent loop starts.
        if let Some(ref registry) = self.extension_registry {
            let state = self.agent.state().await;
            let _ = crate::core::extensions::dispatcher::dispatch_before_agent_start(
                crate::core::extensions::dispatcher::DispatchBeforeAgentStartParams {
                    registry,
                    system_prompt: &state.system_prompt,
                    messages: &state.messages,
                    ext_ctx: &self.ext_ctx,
                },
            ).await;
        }

        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User { content, timestamp };
        // User message is persisted by the event subscriber on MessageEnd
        if let Ok(mut mgr) = self.session_manager.lock() {
            mgr.set_run_prompt(&text);
        }
        self.agent.process(vec![message]).await.ok();
    }

    pub async fn add_user_text(&mut self, text: &str) {
        // Dispatch input event to extensions before processing.
        // If an extension handles the input, skip processing entirely.
        // If an extension transforms the text, use the transformed text.
        let (effective_text, effective_images) = if let Some(ref registry) = self.extension_registry {
            match crate::core::extensions::dispatcher::dispatch_input(
                crate::core::extensions::dispatcher::DispatchInputParams {
                    registry,
                    text,
                    source: "interactive",
                    images: None,
                    ext_ctx: &self.ext_ctx,
                },
            ).await {
                crate::core::extensions::dispatcher::InputEventResult::Handled => return,
                crate::core::extensions::dispatcher::InputEventResult::Continue { text: t, images } => (t, images),
            }
        } else {
            (text.to_string(), Vec::new())
        };

        // Dispatch before_agent_start to extensions before the agent loop starts.
        // Extensions can modify the context (system prompt, messages, tools).
        if let Some(ref registry) = self.extension_registry {
            let state = self.agent.state().await;
            let _ = crate::core::extensions::dispatcher::dispatch_before_agent_start(
                crate::core::extensions::dispatcher::DispatchBeforeAgentStartParams {
                    registry,
                    system_prompt: &state.system_prompt,
                    messages: &state.messages,
                    ext_ctx: &self.ext_ctx,
                },
            ).await;
        }

        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut content = vec![ContentBlock::text(&effective_text)];
        content.extend(effective_images);
        let message = AgentMessage::User {
            content,
            timestamp,
        };
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
    /// Dispatches `model_select` to extensions.
    pub async fn set_model(&mut self, model: Model) {
        let model_id = model.id.clone();
        let mut state = self.agent.state().await;
        let previous_model_id = state.model.id.clone();
        state.model = model;
        drop(state);
        // Dispatch model_select to extensions (fire-and-forget)
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_model_select(
                crate::core::extensions::dispatcher::DispatchModelSelectParams {
                    registry,
                    model: &model_id,
                    previous_model: Some(&previous_model_id),
                    ext_ctx: &self.ext_ctx,
                },
            )
            .await;
        }
    }

    /// Set the thinking level on the agent.
    /// Dispatches `thinking_level_select` to extensions.
    pub async fn set_thinking_level(&mut self, level: &str) {
        let previous_level = self.agent.state().await.thinking_level.clone();
        let mut state = self.agent.state().await;
        state.thinking_level = level.to_string();
        drop(state);
        // Dispatch thinking_level_select to extensions (fire-and-forget)
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_thinking_level_select(
                crate::core::extensions::dispatcher::DispatchThinkingLevelSelectParams {
                    registry,
                    level,
                    previous_level: &previous_level,
                    ext_ctx: &self.ext_ctx,
                },
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

    /// Check whether compaction should be triggered, using token estimation.
    /// Returns true if the context is above the threshold.
    pub async fn check_auto_compact(&self) -> bool {
        use crate::core::compaction;

        let messages = self.agent.state().await.messages;
        let total_tokens = compaction::estimate_agent_messages_tokens(&messages);
        let context_window = 128_000;

        compaction::should_compact(total_tokens, context_window, &self.compaction_settings)
    }

    /// Trigger compaction, matching the original compact().
    /// Returns a summary string on success.
    pub async fn compact(&self, custom_instructions: Option<&str>) -> Result<String, String> {
        use crate::core::compaction;

        // Dispatch session_before_compact to extensions.
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_session_before_compact(
                registry, if custom_instructions.is_some() { "manual" } else { "auto" }, &self.ext_ctx,
            ).await;
        }

        let messages = self.agent.state().await.messages;
        let total_tokens = compaction::estimate_agent_messages_tokens(&messages);
        let context_window = 128_000;

        if !compaction::should_compact(total_tokens, context_window, &self.compaction_settings) {
            return Err("Compaction not needed".to_string());
        }

        let keep_recent_turns = 5usize;
        let cut_point = compaction::find_compaction_cut_point(&messages, keep_recent_turns);

        let prepared = compaction::prepare_compaction(&messages, keep_recent_turns, self.compaction_settings.clone());

        // Build the summarization prompt
        let summarization_prompt = compaction::build_summarization_prompt(
            &prepared.messages_to_summarize,
            prepared.previous_summary.as_deref(),
            custom_instructions,
        );

        // Generate summary using the LLM if a stream_fn is available
        let summary = if let Some(stream_fn) = self.agent.get_stream_fn() {
            let model = self.agent.state().await.model;
            let llm_context = pi_agent_core::pi_ai_types::Context {
                system_prompt: Some(compaction::SUMMARIZATION_SYSTEM_PROMPT.to_string()),
                messages: vec![pi_agent_core::pi_ai_types::Message::User {
                    content: vec![pi_agent_core::pi_ai_types::ContentBlock::text(&summarization_prompt)],
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }],
                tools: None,
            };
            match stream_fn(
                model,
                llm_context,
                None,
                pi_agent_core::types::StreamFnOptions::default(),
            ).await {
                Ok(mut stream) => {
                    use futures::StreamExt;
                    let mut full_text = String::new();
                    while let Some(event) = stream.next().await {
                        match &event {
                            pi_agent_core::pi_ai_types::AssistantMessageEvent::TextDelta { delta, .. } => {
                                full_text.push_str(delta);
                            }
                            pi_agent_core::pi_ai_types::AssistantMessageEvent::Done { message, .. } => {
                                // Use the final message content if we have no deltas
                                if full_text.is_empty() {
                                    for block in &message.content {
                                        if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = block {
                                            full_text.push_str(text);
                                        }
                                    }
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                    if full_text.is_empty() {
                        format!("Compacted {} messages (summary generation unavailable)", messages.len())
                    } else {
                        full_text
                    }
                }
                Err(_) => {
                    format!("Compacted {} messages (LLM unavailable)", messages.len())
                }
            }
        } else {
            format!("Compacted {} messages", messages.len())
        };

        // Record compaction in session manager
        {
            let mut mgr = self.session_manager.lock().unwrap();
            mgr.append_compaction(&summary, &cut_point.first_kept_entry_index.to_string(), total_tokens, None, None);
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
            ).await;
        }

        Ok(summary)
    }

    // =========================================================================
    // Tree Navigation
    // =========================================================================

    /// Navigate the session tree, matching the original navigateTree().
    /// `direction` can be "up", "down", "root", or an entry ID.
    pub fn navigate_tree(&mut self, direction: &str) -> bool {
        let mut mgr = self.session_manager.lock().unwrap();
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
    }

    /// Get the session tree, matching the original getTree().
    pub fn get_tree(&self) -> Vec<crate::core::session_manager::SessionTreeNode> {
        self.session_manager.lock().unwrap().get_tree()
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

    /// Retry the last turn, matching original retry().
    /// Returns the new messages on success.
    pub async fn retry(&self) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        self.agent.continue_run().await
    }

    // =========================================================================
    // Export
    // =========================================================================

    /// Export the session as HTML, matching the original exportHTML().
    /// Returns the HTML content as a string.
    pub fn export_html(&self) -> String {
        let mgr = self.session_manager.lock().unwrap();
        let entries = mgr.get_entries();
        let session_name = mgr.get_session_name().unwrap_or_else(|| "Session".to_string());
        let session_id = mgr.get_session_id();
        let cwd = mgr.get_cwd();

        let mut html = String::new();
        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
        html.push_str("<meta charset=\"UTF-8\">\n");
        html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n");
        html.push_str(&format!("<title>{}</title>\n", html_escape(&session_name)));
        html.push_str("<style>\n");
        html.push_str("body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 20px; background: #fff; color: #333; }\n");
        html.push_str(".message { margin: 12px 0; padding: 12px; border-radius: 8px; }\n");
        html.push_str(".message.user { background: #f0f7ff; border-left: 3px solid #4a90d9; }\n");
        html.push_str(".message.assistant { background: #f5f5f5; border-left: 3px solid #6b7280; }\n");
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
        html.push_str(&format!("<div class=\"meta\">Session: {} | CWD: {}</div>\n", html_escape(session_id), html_escape(cwd)));
        html.push_str("</div>\n");

        // Messages
        for entry in entries {
            match entry {
                crate::core::session_manager::SessionEntry::Message { message, timestamp, .. } => {
                    let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("unknown");
                    let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let css_class = match role {
                        "user" => "user",
                        "assistant" => "assistant",
                        "toolResult" | "tool_result" => "tool",
                        _ => "",
                    };
                    html.push_str(&format!("<div class=\"message {}\">\n", css_class));
                    html.push_str(&format!("<div class=\"role\">{}</div>\n", html_escape(role)));
                    html.push_str(&format!("<div class=\"content\">{}</div>\n", html_escape(content)));
                    html.push_str(&format!("<div class=\"timestamp\">{}</div>\n", html_escape(timestamp)));
                    html.push_str("</div>\n");
                }
                crate::core::session_manager::SessionEntry::Compaction { summary, timestamp, .. } => {
                    html.push_str("<div class=\"message\" style=\"background: #fffbeb; border-left: 3px solid #f59e0b;\">\n");
                    html.push_str("<div class=\"role\">Compaction</div>\n");
                    html.push_str(&format!("<div class=\"content\">{}</div>\n", html_escape(summary)));
                    html.push_str(&format!("<div class=\"timestamp\">{}</div>\n", html_escape(timestamp)));
                    html.push_str("</div>\n");
                }
                crate::core::session_manager::SessionEntry::BranchSummary { summary, timestamp, .. } => {
                    html.push_str("<div class=\"message\" style=\"background: #f0fdf4; border-left: 3px solid #22c55e;\">\n");
                    html.push_str("<div class=\"role\">Branch Summary</div>\n");
                    html.push_str(&format!("<div class=\"content\">{}</div>\n", html_escape(summary)));
                    html.push_str(&format!("<div class=\"timestamp\">{}</div>\n", html_escape(timestamp)));
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
            let mgr = self.session_manager.lock().unwrap();
            let session_id = mgr.get_session_id();
            format!("session_{}.html", session_id)
        });
        std::fs::write(&path, &html).map_err(|e| format!("Failed to write HTML file: {}", e))?;
        Ok(path)
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

impl AgentSession {
    // =========================================================================
    // Session Lifecycle (switch / new / fork / import)
    //
    // These methods operate at the session-manager level. For the full
    // lifecycle management with extension events and factory-based creation,
    // use AgentSessionRuntime instead.
    // =========================================================================

    /// Switch to a different session file, matching original switchSession().
    ///
    /// Dispatches `session_before_switch` to extensions before the switch.
    /// When used through AgentSessionRuntime, the Runtime handles extension
    /// events and factory-based session creation instead.
    pub async fn switch_session(
        &mut self,
        session_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<(), String> {
        use crate::core::session_manager::SessionManager as SM;

        let path = std::path::Path::new(session_path);
        if !path.exists() {
            return Err(format!("Session file not found: {}", session_path));
        }
        if !crate::core::session_manager::is_valid_session_file(path) {
            return Err(format!("Invalid session file: {}", session_path));
        }

        // Dispatch session_before_switch to extensions
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_session_before_switch(
                registry, session_path, &self.ext_ctx,
            ).await;
        }

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
    ///
    /// Dispatches `session_before_fork` to extensions before the fork.
    /// When used through AgentSessionRuntime, the Runtime handles extension
    /// events and factory-based session creation instead.
    pub async fn fork_session(&mut self, entry_id: &str) -> Result<String, String> {
        // Dispatch session_before_fork to extensions
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_session_before_fork(
                registry, entry_id, &self.ext_ctx,
            ).await;
        }

        // Use create_branched_session to create the fork
        let branch_path = self.session_manager.lock().unwrap()
            .create_branched_session(entry_id, None)?;

        // Switch to the new session
        self.switch_session(&branch_path, None).await?;
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

    /// Invalidate the extension context, marking it as stale.
    /// Called by AgentSessionRuntime during session replacement.
    pub fn invalidate_ext_ctx(&self) {
        self.ext_ctx.invalidate();
    }

    /// Execute a bash command directly, matching the original executeBash().
    /// Returns the command output as a string.
    pub async fn execute_bash(&self, command: &str) -> Result<String, String> {
        use crate::core::bash_executor::BashExecutor;
        let executor = BashExecutor::new(&self.cwd);
        let result = executor.execute(command, None).await.map_err(|e| e.to_string())?;
        Ok(result.output)
    }

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

    /// Dispose the session, dispatching session_shutdown to extensions.
    ///
    /// Note: When used through AgentSessionRuntime, the session_shutdown event
    /// is dispatched by the Runtime's teardown_current BEFORE dispose() is
    /// called, so there is no double-dispatch. When called directly (e.g. from
    /// RPC handler or interactive mode), this method dispatches the event.
    pub async fn dispose(self) {
        // Dispatch session_shutdown to extensions so they can flush state
        // and close connections before the session is destroyed.
        if let Some(ref registry) = self.extension_registry {
            crate::core::extensions::dispatcher::dispatch_session_shutdown(
                registry, "quit", &self.ext_ctx,
            ).await;
        }
        // ExtensionRegistry is just a container of trait objects — no V8 thread
        // to stop. Drop is sufficient for cleanup.
    }
}
