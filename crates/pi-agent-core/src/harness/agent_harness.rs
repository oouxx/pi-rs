use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::Future;
use tokio::sync::RwLock;

use crate::harness::types::{
    AbortResult, AgentHarnessOwnEvent, AgentHarnessPhase, AgentHarnessResources,
    AgentHarnessStreamOptions, BeforeAgentStartHookResult, BranchSummaryEntry,
    CompactResult, GenerateBranchSummaryOptions, HarnessError,
    NavigateTreeResult, PendingSessionWrite, PromptTemplate, QueueMode, Session,
    Skill, clone_stream_options, create_failure_message, create_user_message, merge_headers,
};
use crate::pi_ai_types::{ContentBlock, Model, ThinkingLevel};
use crate::types::{AgentContext, AgentEvent, AgentMessage, StreamFn, StreamFnOptions};

// ============================================================
// Type aliases
// ============================================================

type HarnessListener<S, P> = Box<
    dyn Fn(
            AgentHarnessEvent<S, P>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

type HarnessHookHandler = Arc<
    dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Option<serde_json::Value>> + Send>>
        + Send
        + Sync,
>;

// ============================================================
// AgentHarnessEvent — re-exported from here
// ============================================================

#[derive(Debug, Clone)]
pub enum AgentHarnessEvent<S: Clone = Skill, P: Clone = PromptTemplate> {
    Agent(AgentEvent),
    Own(AgentHarnessOwnEvent<S, P>),
}

// ============================================================
// TurnState — private struct for per-turn data
// ============================================================

struct TurnState<S: Clone, P: Clone> {
    messages: Vec<AgentMessage>,
    resources: AgentHarnessResources<S, P>,
    stream_options: AgentHarnessStreamOptions,
    session_id: String,
    system_prompt: String,
    model: Model,
    thinking_level: ThinkingLevel,
}

// ============================================================
// AgentHarness
// ============================================================

pub struct AgentHarness<
    S: Clone + Send + Sync + 'static = Skill,
    P: Clone + Send + Sync + 'static = PromptTemplate,
> {
    // Core state
    session: Arc<RwLock<Session>>,
    model: Arc<RwLock<Model>>,
    thinking_level: Arc<RwLock<ThinkingLevel>>,
    resources: Arc<RwLock<AgentHarnessResources<S, P>>>,
    stream_options: Arc<RwLock<AgentHarnessStreamOptions>>,
    system_prompt_factory: Arc<RwLock<Option<SystemPromptProvider<S, P>>>>,
    get_api_key_and_headers: Arc<RwLock<Option<GetApiKeyAndHeadersFn>>>,

    // Tools
    tools: Arc<RwLock<Vec<String>>>,
    active_tool_names: Arc<RwLock<Vec<String>>>,

    // Queues
    steering_mode: Arc<RwLock<QueueMode>>,
    follow_up_mode: Arc<RwLock<QueueMode>>,
    steer_queue: Arc<RwLock<Vec<AgentMessage>>>,
    follow_up_queue: Arc<RwLock<Vec<AgentMessage>>>,
    next_turn_queue: Arc<RwLock<Vec<AgentMessage>>>,

    // Session writes deferred during active turns
    pending_session_writes: Arc<RwLock<Vec<PendingSessionWrite>>>,

    // Event system
    listeners: Arc<RwLock<Vec<HarnessListener<S, P>>>>,
    handlers: Arc<RwLock<HashMap<String, Vec<HarnessHookHandler>>>>,

    // Phase & run lifecycle
    phase: Arc<RwLock<AgentHarnessPhase>>,
    idle_notify: tokio::sync::watch::Sender<bool>,
    abort_signal: Arc<RwLock<Option<tokio_util::sync::CancellationToken>>>,
}

// ============================================================
// Type aliases for option/providers
// ============================================================

pub type SystemPromptProvider<S, P> =
    Arc<dyn Fn(SystemPromptContext<S, P>) -> Pin<Box<dyn Future<Output = String> + Send>> + Send + Sync>;

#[derive(Debug, Clone)]
pub struct SystemPromptContext<S: Clone = Skill, P: Clone = PromptTemplate> {
    pub env: crate::harness::types::ExecutionEnvInfo,
    pub session: crate::harness::types::SessionInfo,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub active_tools: Vec<String>,
    pub resources: AgentHarnessResources<S, P>,
}

pub type GetApiKeyAndHeadersFn = Arc<
    dyn Fn(&Model) -> Pin<Box<dyn Future<Output = Option<(String, Option<HashMap<String, String>>)>> + Send>>
        + Send
        + Sync,
>;

// ============================================================
// Helper trait for accessing .name on generic Skill/PromptTemplate
// ============================================================

pub trait Named {
    fn name(&self) -> &str;
}

impl Named for Skill {
    fn name(&self) -> &str {
        &self.name
    }
}

impl Named for PromptTemplate {
    fn name(&self) -> &str {
        &self.name
    }
}

// ============================================================
// impl AgentHarness
// ============================================================

impl<S: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> AgentHarness<S, P>
where
    S: serde::Serialize + serde::de::DeserializeOwned,
    P: serde::Serialize + serde::de::DeserializeOwned,
{
    pub fn new(
        session: Session,
        model: Model,
        options: Option<AgentHarnessOptions<S, P>>,
    ) -> Self {
        let opts = options.unwrap_or_default();

        let tool_names: Vec<String> = opts.tools.unwrap_or_default();
        let active_tool_names = opts
            .active_tool_names
            .clone()
            .unwrap_or_else(|| tool_names.clone());

        Self {
            session: Arc::new(RwLock::new(session)),
            model: Arc::new(RwLock::new(model)),
            thinking_level: Arc::new(RwLock::new(
                opts.thinking_level.unwrap_or_else(|| "off".to_string()),
            )),
            resources: Arc::new(RwLock::new(opts.resources.unwrap_or(
                AgentHarnessResources {
                    skills: None,
                    prompt_templates: None,
                },
            ))),
            stream_options: Arc::new(RwLock::new(opts.stream_options.unwrap_or(
                AgentHarnessStreamOptions {
                    temperature: None,
                    top_p: None,
                    max_tokens: None,
                    transport: None,
                    timeout_ms: None,
                    max_retries: None,
                    max_retry_delay_ms: None,
                    cache_retention: None,
                    headers: None,
                    metadata: None,
                },
            ))),
            system_prompt_factory: Arc::new(RwLock::new(opts.system_prompt_factory)),
            get_api_key_and_headers: Arc::new(RwLock::new(opts.get_api_key_and_headers)),
            tools: Arc::new(RwLock::new(tool_names)),
            active_tool_names: Arc::new(RwLock::new(active_tool_names)),
            steering_mode: Arc::new(RwLock::new(opts.steering_mode.unwrap_or(QueueMode::Queue))),
            follow_up_mode: Arc::new(RwLock::new(
                opts.follow_up_mode.unwrap_or(QueueMode::Queue),
            )),
            steer_queue: Arc::new(RwLock::new(Vec::new())),
            follow_up_queue: Arc::new(RwLock::new(Vec::new())),
            next_turn_queue: Arc::new(RwLock::new(Vec::new())),
            pending_session_writes: Arc::new(RwLock::new(Vec::new())),
            listeners: Arc::new(RwLock::new(Vec::new())),
            handlers: Arc::new(RwLock::new(HashMap::new())),
            phase: Arc::new(RwLock::new(AgentHarnessPhase::Idle)),
            idle_notify: tokio::sync::watch::channel(true).0,
            abort_signal: Arc::new(RwLock::new(None)),
        }
    }

    // ========================================================
    // Getters / Setters
    // ========================================================

    pub async fn model(&self) -> Model {
        self.model.read().await.clone()
    }

    pub async fn set_model(&self, model: Model) -> Result<(), HarnessError> {
        let previous = self.model.read().await.clone();
        {
            let phase = *self.phase.read().await;
            if phase == AgentHarnessPhase::Idle {
                let mut session = self.session.write().await;
                session
                    .append_model_change(model.provider.clone(), model.id.clone())
                    .await
                    .map_err(HarnessError::Session)?;
            } else {
                self.pending_session_writes.write().await.push(
                    PendingSessionWrite::ModelChange {
                        provider: model.provider.clone(),
                        model_id: model.id.clone(),
                    },
                );
            }
        }
        *self.model.write().await = model.clone();
        self.emit_own(AgentHarnessOwnEvent::ModelUpdate {
            model,
            previous_model: previous,
            source: "set".into(),
        })
        .await;
        Ok(())
    }

    pub async fn thinking_level(&self) -> ThinkingLevel {
        self.thinking_level.read().await.clone()
    }

    pub async fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), HarnessError> {
        let previous = self.thinking_level.read().await.clone();
        {
            let phase = *self.phase.read().await;
            if phase == AgentHarnessPhase::Idle {
                let mut session = self.session.write().await;
                session
                    .append_thinking_level_change(level.clone())
                    .await
                    .map_err(HarnessError::Session)?;
            } else {
                self.pending_session_writes
                    .write()
                    .await
                    .push(PendingSessionWrite::ThinkingLevelChange {
                        thinking_level: level.clone(),
                    });
            }
        }
        *self.thinking_level.write().await = level.clone();
        self.emit_own(AgentHarnessOwnEvent::ThinkingLevelUpdate {
            level,
            previous_level: previous,
        })
        .await;
        Ok(())
    }

    pub async fn get_tools(&self) -> Vec<String> {
        self.tools.read().await.clone()
    }

    pub async fn set_tools(&self, tools: Vec<String>) -> Result<(), HarnessError> {
        let previous_tool_names = self.tools.read().await.clone();
        let previous_active_tool_names = self.active_tool_names.read().await.clone();
        let active_tool_names = previous_active_tool_names
            .iter()
            .filter(|n| tools.contains(n))
            .cloned()
            .collect::<Vec<_>>();

        {
            let phase = *self.phase.read().await;
            if phase == AgentHarnessPhase::Idle {
                let mut session = self.session.write().await;
                session
                    .append_active_tools_change(active_tool_names.clone())
                    .await
                    .map_err(HarnessError::Session)?;
            } else {
                self.pending_session_writes
                    .write()
                    .await
                    .push(PendingSessionWrite::ActiveToolsChange {
                        active_tool_names: active_tool_names.clone(),
                    });
            }
        }
        *self.tools.write().await = tools.clone();
        *self.active_tool_names.write().await = active_tool_names.clone();
        self.emit_own(AgentHarnessOwnEvent::ToolsUpdate {
            tool_names: tools,
            previous_tool_names: previous_tool_names,
            active_tool_names,
            previous_active_tool_names,
            source: "set".into(),
        })
        .await;
        Ok(())
    }

    pub async fn get_active_tools(&self) -> Vec<String> {
        self.active_tool_names.read().await.clone()
    }

    pub async fn set_active_tools(&self, tool_names: Vec<String>) -> Result<(), HarnessError> {
        self.validate_tool_names(&tool_names).await?;
        let previous = self.active_tool_names.read().await.clone();
        {
            let phase = *self.phase.read().await;
            if phase == AgentHarnessPhase::Idle {
                let mut session = self.session.write().await;
                session
                    .append_active_tools_change(tool_names.clone())
                    .await
                    .map_err(HarnessError::Session)?;
            } else {
                self.pending_session_writes
                    .write()
                    .await
                    .push(PendingSessionWrite::ActiveToolsChange {
                        active_tool_names: tool_names.clone(),
                    });
            }
        }
        let previous_tool_names = self.tools.read().await.clone();
        *self.active_tool_names.write().await = tool_names.clone();
        self.emit_own(AgentHarnessOwnEvent::ToolsUpdate {
            tool_names: self.tools.read().await.clone(),
            previous_tool_names,
            active_tool_names: tool_names,
            previous_active_tool_names: previous,
            source: "set".into(),
        })
        .await;
        Ok(())
    }

    async fn validate_tool_names(&self, _names: &[String]) -> Result<(), HarnessError> {
        let _tools = self.tools.read().await;
        Ok(())
    }

    pub async fn steering_mode(&self) -> QueueMode {
        *self.steering_mode.read().await
    }

    pub async fn set_steering_mode(&self, mode: QueueMode) {
        *self.steering_mode.write().await = mode;
    }

    pub async fn follow_up_mode(&self) -> QueueMode {
        *self.follow_up_mode.read().await
    }

    pub async fn set_follow_up_mode(&self, mode: QueueMode) {
        *self.follow_up_mode.write().await = mode;
    }

    pub async fn get_resources(&self) -> AgentHarnessResources<S, P> {
        self.resources.read().await.clone()
    }

    pub async fn set_resources(&self, resources: AgentHarnessResources<S, P>) {
        let previous = self.get_resources().await;
        *self.resources.write().await = resources.clone();
        self.emit_own(AgentHarnessOwnEvent::ResourcesUpdate {
            resources,
            previous_resources: previous,
        })
        .await;
    }

    pub async fn get_stream_options(&self) -> AgentHarnessStreamOptions {
        clone_stream_options(&*self.stream_options.read().await)
    }

    pub async fn set_stream_options(&self, options: AgentHarnessStreamOptions) {
        *self.stream_options.write().await = options;
    }

    // ========================================================
    // Event system
    // ========================================================

    pub async fn subscribe(&self, listener: HarnessListener<S, P>) {
        self.listeners.write().await.push(listener);
    }

    pub async fn on<F>(&self, event_type: &str, handler: F)
    where
        F: Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Option<serde_json::Value>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let mut handlers = self.handlers.write().await;
        handlers
            .entry(event_type.to_string())
            .or_default()
            .push(Arc::new(handler));
    }

    async fn emit_own(&self, event: AgentHarnessOwnEvent<S, P>) {
        let listeners = self.listeners.read().await;
        let harness_event = AgentHarnessEvent::Own(event);
        for listener in listeners.iter() {
            listener(harness_event.clone()).await;
        }
    }

    #[allow(dead_code)]
    async fn emit_any(&self, event: AgentHarnessEvent<S, P>) {
        let listeners = self.listeners.read().await;
        for listener in listeners.iter() {
            listener(event.clone()).await;
        }
    }

    async fn emit_hook(&self, event_type: &str, payload: serde_json::Value) -> Option<serde_json::Value> {
        let handlers = self.handlers.read().await;
        let event_handlers = handlers.get(event_type)?;
        let mut last_result: Option<serde_json::Value> = None;
        for handler in event_handlers.iter() {
            if let Some(result) = handler(payload.clone()).await {
                last_result = Some(result);
            }
        }
        last_result
    }

    // ========================================================
    // Queue operations
    // ========================================================

    async fn emit_queue_update(&self) {
        self.emit_own(AgentHarnessOwnEvent::QueueUpdate {
            steer_queue: self.steer_queue.read().await.clone(),
            follow_up_queue: self.follow_up_queue.read().await.clone(),
            next_turn_queue: self.next_turn_queue.read().await.clone(),
        })
        .await;
    }

    // ========================================================
    // Steer / FollowUp / NextTurn
    // ========================================================

    pub async fn steer(&self, text: &str, images: Option<Vec<ContentBlock>>) -> Result<(), HarnessError> {
        if *self.phase.read().await == AgentHarnessPhase::Idle {
            return Err(HarnessError::InvalidState(
                "Cannot steer while idle".into(),
            ));
        }
        let mode = *self.steering_mode.read().await;
        let mut queue = self.steer_queue.write().await;
        let msg = create_user_message(text, images);
        match mode {
            QueueMode::Queue => queue.push(msg),
            QueueMode::Replace => {
                queue.clear();
                queue.push(msg);
            }
            QueueMode::Drop => {
                if queue.is_empty() {
                    queue.push(msg);
                }
            }
        }
        drop(queue);
        self.emit_queue_update().await;
        Ok(())
    }

    pub async fn follow_up(&self, text: &str, images: Option<Vec<ContentBlock>>) -> Result<(), HarnessError> {
        if *self.phase.read().await == AgentHarnessPhase::Idle {
            return Err(HarnessError::InvalidState(
                "Cannot follow up while idle".into(),
            ));
        }
        let mode = *self.follow_up_mode.read().await;
        let mut queue = self.follow_up_queue.write().await;
        let msg = create_user_message(text, images);
        match mode {
            QueueMode::Queue => queue.push(msg),
            QueueMode::Replace => {
                queue.clear();
                queue.push(msg);
            }
            QueueMode::Drop => {
                if queue.is_empty() {
                    queue.push(msg);
                }
            }
        }
        drop(queue);
        self.emit_queue_update().await;
        Ok(())
    }

    pub async fn next_turn(&self, text: &str, images: Option<Vec<ContentBlock>>) -> Result<(), HarnessError> {
        self.next_turn_queue
            .write()
            .await
            .push(create_user_message(text, images));
        self.emit_queue_update().await;
        Ok(())
    }

    // ========================================================
    // Append message
    // ========================================================

    pub async fn append_message(&self, message: AgentMessage) -> Result<(), HarnessError> {
        let phase = *self.phase.read().await;
        if phase == AgentHarnessPhase::Idle {
            let mut session = self.session.write().await;
            session
                .append_message(message)
                .await
                .map_err(HarnessError::Session)?;
        } else {
            self.pending_session_writes
                .write()
                .await
                .push(PendingSessionWrite::Message { message });
        }
        Ok(())
    }

    // ========================================================
    // Create turn state
    // ========================================================

    async fn create_turn_state(&self) -> Result<TurnState<S, P>, HarnessError> {
        let context = self
            .session
            .read()
            .await
            .build_context()
            .await
            .map_err(HarnessError::Session)?;
        let resources = self.get_resources().await;
        let metadata = self.session.read().await.get_metadata().await;
        let model = self.model.read().await.clone();
        let thinking_level = self.thinking_level.read().await.clone();
        let active_tools = self.active_tool_names.read().await.clone();
        let stream_options = self.get_stream_options().await;

        // Resolve system prompt
        let system_prompt = {
            let factory = self.system_prompt_factory.read().await;
            match factory.as_ref() {
                Some(f) => {
                    let ctx = SystemPromptContext {
                        env: crate::harness::types::ExecutionEnvInfo {
                            cwd: String::new(),
                        },
                        session: crate::harness::types::SessionInfo {
                            id: metadata.id.clone(),
                            created_at: metadata.created_at.clone(),
                        },
                        model: model.clone(),
                        thinking_level: thinking_level.clone(),
                        active_tools: active_tools.clone(),
                        resources: resources.clone(),
                    };
                    f(ctx).await
                }
                None => "You are a helpful assistant.".to_string(),
            }
        };

        Ok(TurnState {
            messages: context.messages,
            resources,
            stream_options,
            session_id: metadata.id,
            system_prompt,
            model,
            thinking_level,
        })
    }

    // ========================================================
    // Execute turn
    // ========================================================

    async fn execute_turn(
        &self,
        turn_state: TurnState<S, P>,
        text: String,
        images: Option<Vec<ContentBlock>>,
    ) -> Result<AgentMessage, HarnessError> {
        let mut messages: Vec<AgentMessage> = vec![create_user_message(&text, images)];

        // Drain next_turn_queue
        {
            let mut next_queue = self.next_turn_queue.write().await;
            if !next_queue.is_empty() {
                let queued: Vec<AgentMessage> = next_queue.drain(..).collect();
                drop(next_queue);
                self.emit_queue_update().await;
                messages = [queued, messages].concat();
            }
        }

        // Emit before_agent_start hook
        let hook_payload = serde_json::json!({
            "prompt": text,
            "system_prompt": turn_state.system_prompt,
        });
        let hook_result: Option<BeforeAgentStartHookResult> = self
            .emit_hook("before_agent_start", hook_payload)
            .await
            .and_then(|v| serde_json::from_value(v).ok());

        if let Some(ref result) = hook_result {
            if let Some(ref extra_msgs) = result.messages {
                let mut extended = extra_msgs.clone();
                extended.extend(messages);
                messages = extended;
            }
        }

        let system_prompt = hook_result
            .as_ref()
            .and_then(|r| r.system_prompt.clone())
            .unwrap_or(turn_state.system_prompt.clone());

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let cancel_clone = cancel_token.clone();
        tokio::spawn(async move {
            cancel_clone.cancelled().await;
            let _ = cancel_tx.send(true);
        });
        *self.abort_signal.write().await = Some(cancel_token);

        let context = AgentContext {
            system_prompt,
            messages: turn_state.messages.clone(),
            tools: None,
        };

        let get_ak_ah = self.get_api_key_and_headers.clone();
        let stream_snapshot = turn_state.stream_options;
        let session_id = turn_state.session_id.clone();
        let session_id2 = session_id.clone();
        let model = turn_state.model.clone();

        let stream_fn: StreamFn = Arc::new(move |model: Model, ctx: crate::pi_ai_types::Context, reasoning: Option<ThinkingLevel>, opts: StreamFnOptions| {
            let get_ak_ah = get_ak_ah.clone();
            let mut snapshot = clone_stream_options(&stream_snapshot);
            let session_id = session_id.clone();

            Box::pin(async move {
                let auth = {
                    let get_auth = get_ak_ah.read().await;
                    match get_auth.as_ref() {
                        Some(f) => f(&model).await,
                        None => None,
                    }
                };
                if let Some((_api_key, auth_headers)) = &auth {
                    snapshot.headers = merge_headers(&[snapshot.headers.clone(), auth_headers.clone()]);
                }

                // Build SimpleStreamOptions from our data
                let simple_opts = pi_ai::types::SimpleStreamOptions {
                    base: pi_ai::types::StreamOptions {
                        temperature: snapshot.temperature,
                        max_tokens: snapshot.max_tokens,
                        signal: opts.signal.clone(),
                        api_key: auth.as_ref().map(|(k, _)| k.clone()),
                        transport: snapshot.transport.clone().map(|_| pi_ai::types::Transport::Websocket),
                        cache_retention: snapshot.cache_retention.clone().and_then(|s| match s.to_lowercase().as_str() {
                            "short" => Some(pi_ai::types::CacheRetention::Short),
                            "long" => Some(pi_ai::types::CacheRetention::Long),
                            _ => None,
                        }),
                        session_id: Some(session_id.clone()),
                        headers: snapshot.headers.clone(),
                        timeout_ms: snapshot.timeout_ms,
                        websocket_connect_timeout_ms: None,
                        max_retries: snapshot.max_retries,
                        max_retry_delay_ms: snapshot.max_retry_delay_ms,
                        metadata: None,
                        tool_choice: None,
                    },
                    reasoning: reasoning.clone(),
                    thinking_budgets: opts.thinking_budgets.clone(),
                };

                let event_stream = pi_ai::stream::stream_simple(&model, &ctx, Some(simple_opts));

                // Bridge AssistantMessageEventStream (not Unpin) to StreamResponse (Unpin)
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<crate::pi_ai_types::AssistantMessageEvent>();
                tokio::spawn(async move {
                    use futures::StreamExt;
                    let mut stream = event_stream;
                    while let Some(event) = stream.next().await {
                        let _ = tx.send(event);
                    }
                });

                let stream_response: crate::pi_ai_types::StreamResponse =
                    Box::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));

                Ok(stream_response)
            })
        });

        let emit = self.create_event_sink();

        let steer_queue = self.steer_queue.clone();
        let steer_mode = *self.steering_mode.read().await;
        let follow_up_queue = self.follow_up_queue.clone();
        let follow_up_mode = *self.follow_up_mode.read().await;
        let handlers = self.handlers.clone();
        let handlers2 = handlers.clone();
        let handlers3 = handlers.clone();

        let config = crate::agent_loop::AgentLoopConfig {
            model: model.clone(),
            reasoning: if turn_state.thinking_level == "off" {
                None
            } else {
                Some(turn_state.thinking_level.clone())
            },
            api_key: None,
            session_id: Some(session_id2.clone()),
            thinking_budgets: None,
            transport: None,
            max_retry_delay_ms: None,
            tool_execution: crate::pi_ai_types::ToolExecutionMode::Parallel,
            convert_to_llm: Arc::new(crate::harness::messages::convert_to_llm),
            transform_context: Some(Arc::new(move |msgs: Vec<AgentMessage>, _signal: Option<tokio::sync::watch::Receiver<bool>>| {
                let handlers = handlers.clone();
                Box::pin(async move {
                    let payload = serde_json::json!({ "messages": msgs });
                    let result = {
                        let h = handlers.read().await;
                        let event_handlers = h.get("context");
                        let mut last: Option<Vec<AgentMessage>> = None;
                        if let Some(eh) = event_handlers {
                            for handler in eh.iter() {
                                if let Some(res) = handler(payload.clone()).await {
                                    if let Ok(parsed) = serde_json::from_value::<serde_json::Value>(res) {
                                        if let Some(msgs) = parsed.get("messages").and_then(|v| serde_json::from_value(v.clone()).ok()) {
                                            last = Some(msgs);
                                        }
                                    }
                                }
                            }
                        }
                        last
                    };
                    result.unwrap_or(msgs)
                })
            })),
            get_api_key: None,
            get_steering_messages: Some(Arc::new(move || {
                let q = steer_queue.clone();
                let mode = steer_mode;
                Box::pin(async move {
                    let mut guard = q.write().await;
                    let msgs: Vec<AgentMessage> = match mode {
                        QueueMode::Queue => guard.drain(..).collect(),
                        QueueMode::Replace => {
                            if guard.is_empty() { Vec::new() } else { vec![guard.remove(0)] }
                        }
                        QueueMode::Drop => {
                            if guard.is_empty() { Vec::new() } else { vec![guard.remove(0)] }
                        }
                    };
                    msgs
                })
            })),
            get_follow_up_messages: Some(Arc::new(move || {
                let q = follow_up_queue.clone();
                let mode = follow_up_mode;
                Box::pin(async move {
                    let mut guard = q.write().await;
                    let msgs: Vec<AgentMessage> = match mode {
                        QueueMode::Queue => guard.drain(..).collect(),
                        QueueMode::Replace => {
                            if guard.is_empty() { Vec::new() } else { vec![guard.remove(0)] }
                        }
                        QueueMode::Drop => {
                            if guard.is_empty() { Vec::new() } else { vec![guard.remove(0)] }
                        }
                    };
                    msgs
                })
            })),
            should_stop_after_turn: None,
            prepare_next_turn: None,
            before_tool_call: Some(Arc::new(move |ctx: crate::types::BeforeToolCallContext, _signal: Option<tokio::sync::watch::Receiver<bool>>| {
                let handlers = handlers2.clone();
                Box::pin(async move {
                    let payload = serde_json::json!({
                        "tool_call_id": ctx.tool_call.id,
                        "tool_name": ctx.tool_call.name,
                        "input": ctx.args,
                    });
                    let result = {
                        let h = handlers.read().await;
                        let event_handlers = h.get("tool_call");
                        let mut last: Option<crate::types::BeforeToolCallResult> = None;
                        if let Some(eh) = event_handlers {
                            for handler in eh.iter() {
                                if let Some(res) = handler(payload.clone()).await {
                                    if let Ok(v) = serde_json::from_value::<serde_json::Value>(res) {
                                        let block = v.get("block").and_then(|b| b.as_bool());
                                        let reason = v.get("reason").and_then(|r| r.as_str().map(String::from));
                                        last = Some(crate::types::BeforeToolCallResult { block: block.unwrap_or(false), reason });
                                    }
                                }
                            }
                        }
                        last
                    };
                    result
                })
            })),
            after_tool_call: Some(Arc::new(move |ctx: crate::types::AfterToolCallContext, _signal: Option<tokio::sync::watch::Receiver<bool>>| {
                let handlers = handlers3.clone();
                Box::pin(async move {
                    let payload = serde_json::json!({
                        "tool_call_id": ctx.tool_call.id,
                        "tool_name": ctx.tool_call.name,
                        "input": ctx.args,
                        "content": ctx.result.content,
                        "details": ctx.result.details,
                        "is_error": ctx.is_error,
                    });
                    let result = {
                        let h = handlers.read().await;
                        let event_handlers = h.get("tool_result");
                        let mut last: Option<crate::types::AfterToolCallResult> = None;
                        if let Some(eh) = event_handlers {
                            for handler in eh.iter() {
                                if let Some(res) = handler(payload.clone()).await {
                                    if let Ok(v) = serde_json::from_value::<serde_json::Value>(res) {
                                        last = Some(crate::types::AfterToolCallResult {
                                            content: v.get("content").and_then(|c| serde_json::from_value(c.clone()).ok()),
                                            details: v.get("details").cloned(),
                                            is_error: v.get("is_error").and_then(|b| b.as_bool()),
                                            terminate: v.get("terminate").and_then(|b| b.as_bool()),
                                        });
                                    }
                                }
                            }
                        }
                        last
                    };
                    result
                })
            })),
            on_payload: None,
            on_response: None,
            max_consecutive_tool_calls: None,
        };

        let signal = Some(cancel_rx);

        let result = crate::agent_loop::run_agent_loop(messages, context, &config, &emit, &signal, &stream_fn).await;

        *self.abort_signal.write().await = None;

        match result {
            Ok(new_messages) => {
                // Flush any remaining pending writes
                self.flush_pending_session_writes().await?;
                // Return the last assistant message
                for msg in new_messages.iter().rev() {
                    if matches!(msg, AgentMessage::Assistant { .. }) {
                        return Ok(msg.clone());
                    }
                }
                Err(HarnessError::InvalidState(
                    "AgentHarness prompt completed without an assistant message".into(),
                ))
            }
            Err(e) => {
                let was_aborted = signal.as_ref().map(|rx| *rx.borrow()).unwrap_or(false);
                let failure_msg = create_failure_message(&model, &e.to_string(), was_aborted);
                let failure_result = self.emit_run_failure(model.clone(), &e.to_string(), was_aborted).await;
                match failure_result {
                    Ok(msgs) => {
                        for msg in msgs.iter().rev() {
                            if matches!(msg, AgentMessage::Assistant { .. }) {
                                return Ok(msg.clone());
                            }
                        }
                        Ok(failure_msg)
                    }
                    Err(_) => Ok(failure_msg),
                }
            }
        }
    }

    // ========================================================
    // Emit run failure
    // ========================================================

    async fn emit_run_failure(
        &self,
        model: Model,
        error: &str,
        aborted: bool,
    ) -> Result<Vec<AgentMessage>, HarnessError> {
        let failure_msg = create_failure_message(&model, error, aborted);
        let emit = self.create_event_sink();
        emit(AgentEvent::MessageStart {
            message: failure_msg.clone(),
        })
        .await;
        emit(AgentEvent::MessageEnd {
            message: failure_msg.clone(),
        })
        .await;
        emit(AgentEvent::TurnEnd {
            message: failure_msg.clone(),
            tool_results: Vec::new(),
        })
        .await;
        emit(AgentEvent::AgentEnd {
            messages: vec![failure_msg.clone()],
        })
        .await;
        Ok(vec![failure_msg])
    }

    // ========================================================
    // Create event sink
    // ========================================================

    fn create_event_sink(&self) -> crate::types::AgentEventSink {
        let session = self.session.clone();
        let pending_writes = self.pending_session_writes.clone();
        let phase = self.phase.clone();
        let idle_notify = self.idle_notify.clone();
        let emit_own = self.create_emit_own_fn();

        Arc::new(move |event: AgentEvent| {
            let session = session.clone();
            let pending_writes = pending_writes.clone();
            let phase = phase.clone();
            let idle_notify = idle_notify.clone();
            let emit_own = emit_own.clone();

            Box::pin(async move {
                match &event {
                    AgentEvent::MessageEnd { message } => {
                        let mut s = session.write().await;
                        let _ = s.append_message(message.clone()).await;
                    }
                    AgentEvent::TurnEnd { .. } => {
                        let mut writes = pending_writes.write().await;
                        while let Some(write) = writes.pop() {
                            let mut s = session.write().await;
                            let _ = match write {
                                PendingSessionWrite::Message { message } => {
                                    s.append_message(message).await.map(|_| ())
                                }
                                PendingSessionWrite::ModelChange {
                                    provider,
                                    model_id,
                                } => s.append_model_change(provider, model_id).await.map(|_| ()),
                                PendingSessionWrite::ThinkingLevelChange {
                                    thinking_level,
                                } => s
                                    .append_thinking_level_change(thinking_level)
                                    .await
                                    .map(|_| ()),
                                PendingSessionWrite::ActiveToolsChange {
                                    active_tool_names,
                                } => s
                                    .append_active_tools_change(active_tool_names)
                                    .await
                                    .map(|_| ()),
                                PendingSessionWrite::Custom {
                                    custom_type,
                                    data,
                                } => s
                                    .append_custom_entry(custom_type, data)
                                    .await
                                    .map(|_| ()),
                                PendingSessionWrite::CustomMessage {
                                    custom_type,
                                    content,
                                    ..
                                } => s
                                    .append_custom_entry(custom_type, Some(content))
                                    .await
                                    .map(|_| ()),
                                PendingSessionWrite::Label { target_id, label } => {
                                    s.append_label(target_id, label).await.map(|_| ())
                                }
                                PendingSessionWrite::SessionInfo { name } => {
                                    s.append_session_name(name).await.map(|_| ())
                                }
                                PendingSessionWrite::Leaf { .. } => Ok(()),
                            };
                        }
                        emit_own(AgentHarnessOwnEvent::SavePoint {
                            had_pending_mutations: false,
                        })
                        .await;
                    }
                    AgentEvent::AgentEnd { .. } => {
                        *phase.write().await = AgentHarnessPhase::Idle;
                        let _ = idle_notify.send(true);
                        emit_own(AgentHarnessOwnEvent::Settled {
                            next_turn_count: 0,
                        })
                        .await;
                    }
                    _ => {}
                }
            })
        })
    }

    fn create_emit_own_fn(
        &self,
    ) -> Arc<dyn Fn(AgentHarnessOwnEvent<S, P>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>
    {
        let listeners = self.listeners.clone();
        Arc::new(move |event: AgentHarnessOwnEvent<S, P>| {
            let listeners = listeners.clone();
            Box::pin(async move {
                let listeners = listeners.read().await;
                let harness_event = AgentHarnessEvent::Own(event);
                for listener in listeners.iter() {
                    listener(harness_event.clone()).await;
                }
            })
        })
    }

    // ========================================================
    // Flush pending session writes
    // ========================================================

    async fn flush_pending_session_writes(&self) -> Result<(), HarnessError> {
        let mut writes = self.pending_session_writes.write().await;
        while let Some(write) = writes.pop() {
            let mut s = self.session.write().await;
            match write {
                PendingSessionWrite::Message { message } => {
                    s.append_message(message).await.map_err(HarnessError::Session)?;
                }
                PendingSessionWrite::ModelChange {
                    provider,
                    model_id,
                } => {
                    s.append_model_change(provider, model_id)
                        .await
                        .map_err(HarnessError::Session)?;
                }
                PendingSessionWrite::ThinkingLevelChange { thinking_level } => {
                    s.append_thinking_level_change(thinking_level)
                        .await
                        .map_err(HarnessError::Session)?;
                }
                PendingSessionWrite::ActiveToolsChange {
                    active_tool_names,
                } => {
                    s.append_active_tools_change(active_tool_names)
                        .await
                        .map_err(HarnessError::Session)?;
                }
                PendingSessionWrite::Custom {
                    custom_type,
                    data,
                } => {
                    s.append_custom_entry(custom_type, data)
                        .await
                        .map_err(HarnessError::Session)?;
                }
                PendingSessionWrite::CustomMessage {
                    custom_type,
                    content,
                    ..
                } => {
                    s.append_custom_entry(custom_type, Some(content))
                        .await
                        .map_err(HarnessError::Session)?;
                }
                PendingSessionWrite::Label { target_id, label } => {
                    s.append_label(target_id, label)
                        .await
                        .map_err(HarnessError::Session)?;
                }
                PendingSessionWrite::SessionInfo { name } => {
                    s.append_session_name(name)
                        .await
                        .map_err(HarnessError::Session)?;
                }
                PendingSessionWrite::Leaf { .. } => {}
            }
        }
        Ok(())
    }

    // ========================================================
    // Public API: prompt
    // ========================================================

    pub async fn prompt(
        &self,
        text: &str,
        images: Option<Vec<ContentBlock>>,
    ) -> Result<AgentMessage, HarnessError> {
        if *self.phase.read().await != AgentHarnessPhase::Idle {
            return Err(HarnessError::Busy("AgentHarness is busy".into()));
        }

        *self.phase.write().await = AgentHarnessPhase::Turn;

        let result = self
            .prompt_inner(text, images)
            .await;

        match result {
            Ok(msg) => {
                *self.phase.write().await = AgentHarnessPhase::Idle;
                Ok(msg)
            }
            Err(e) => {
                *self.phase.write().await = AgentHarnessPhase::Idle;
                Err(e)
            }
        }
    }

    async fn prompt_inner(
        &self,
        text: &str,
        images: Option<Vec<ContentBlock>>,
    ) -> Result<AgentMessage, HarnessError> {
        let turn_state = self.create_turn_state().await?;
        self.execute_turn(turn_state, text.to_string(), images)
            .await
    }
}

impl<S: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> AgentHarness<S, P>
where
    S: serde::Serialize + serde::de::DeserializeOwned + Named,
    P: serde::Serialize + serde::de::DeserializeOwned + Named,
{
    // ========================================================
    // Public API: skill
    // ========================================================

    pub async fn skill(
        &self,
        name: &str,
        additional_instructions: Option<&str>,
    ) -> Result<AgentMessage, HarnessError> {
        if *self.phase.read().await != AgentHarnessPhase::Idle {
            return Err(HarnessError::Busy("AgentHarness is busy".into()));
        }

        let turn_state = self.create_turn_state().await?;
        let skill_entry = turn_state
            .resources
            .skills
            .as_ref()
            .and_then(|skills| skills.iter().find(|s| s.name() == name))
            .cloned()
            .ok_or_else(|| HarnessError::InvalidArgument(format!("Unknown skill: {}", name)))?;

        *self.phase.write().await = AgentHarnessPhase::Turn;

        // Convert generic S to concrete Skill via JSON
        let skill: Skill = serde_json::from_value(serde_json::to_value(&skill_entry).map_err(|_| {
            HarnessError::InvalidArgument("Failed to serialize skill entry".into())
        })?)
        .map_err(|_| HarnessError::InvalidArgument("Failed to deserialize skill".into()))?;

        let text = crate::harness::skills::format_skill_invocation(&skill, additional_instructions);
        let result = self.execute_turn(turn_state, text, None).await;

        match result {
            Ok(msg) => {
                *self.phase.write().await = AgentHarnessPhase::Idle;
                Ok(msg)
            }
            Err(e) => {
                *self.phase.write().await = AgentHarnessPhase::Idle;
                Err(e)
            }
        }
    }

    // ========================================================
    // Public API: prompt_from_template
    // ========================================================

    pub async fn prompt_from_template(
        &self,
        name: &str,
        args: &[String],
    ) -> Result<AgentMessage, HarnessError> {
        if *self.phase.read().await != AgentHarnessPhase::Idle {
            return Err(HarnessError::Busy("AgentHarness is busy".into()));
        }

        let turn_state = self.create_turn_state().await?;
        let template = turn_state
            .resources
            .prompt_templates
            .as_ref()
            .and_then(|templates| templates.iter().find(|t| t.name() == name))
            .cloned()
            .ok_or_else(|| {
                HarnessError::InvalidArgument(format!("Unknown prompt template: {}", name))
            })?;

        *self.phase.write().await = AgentHarnessPhase::Turn;

        // Convert generic P to concrete PromptTemplate via JSON
        let template: PromptTemplate = serde_json::from_value(serde_json::to_value(&template).map_err(|_| {
            HarnessError::InvalidArgument("Failed to serialize prompt template".into())
        })?)
        .map_err(|_| HarnessError::InvalidArgument("Failed to deserialize prompt template".into()))?;

        let text = crate::harness::prompt_templates::format_prompt_template_invocation(
            &template, args,
        );
        let result = self.execute_turn(turn_state, text, None).await;

        match result {
            Ok(msg) => {
                *self.phase.write().await = AgentHarnessPhase::Idle;
                Ok(msg)
            }
            Err(e) => {
                *self.phase.write().await = AgentHarnessPhase::Idle;
                Err(e)
            }
        }
    }
}

impl<S: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> AgentHarness<S, P>
where
    S: serde::Serialize + serde::de::DeserializeOwned,
    P: serde::Serialize + serde::de::DeserializeOwned,
{
    // ========================================================
    // Public API: abort
    // ========================================================

    pub async fn abort(&self) -> Result<AbortResult, HarnessError> {
        let cleared_steer: Vec<AgentMessage> = self.steer_queue.write().await.drain(..).collect();
        let cleared_follow_up: Vec<AgentMessage> = self.follow_up_queue.write().await.drain(..).collect();
        self.next_turn_queue.write().await.clear();

        if let Some(cancel) = self.abort_signal.write().await.take() {
            cancel.cancel();
        }

        let _ = self.emit_queue_update().await;
        self.wait_for_idle().await;

        self.emit_own(AgentHarnessOwnEvent::Abort {
            cleared_steer: cleared_steer.clone(),
            cleared_follow_up: cleared_follow_up.clone(),
        })
        .await;

        Ok(AbortResult {
            cleared_steer,
            cleared_follow_up,
        })
    }

    // ========================================================
    // Public API: wait_for_idle
    // ========================================================

    pub async fn wait_for_idle(&self) {
        let mut rx = self.idle_notify.subscribe();
        let _ = rx.changed().await;
    }

    // ========================================================
    // Public API: compact
    // ========================================================

    pub async fn compact(
        &self,
        custom_instructions: Option<&str>,
    ) -> Result<Option<CompactResult>, HarnessError> {
        if *self.phase.read().await != AgentHarnessPhase::Idle {
            return Err(HarnessError::Busy("compact() requires idle harness".into()));
        }

        *self.phase.write().await = AgentHarnessPhase::Compaction;

        let result = self.compact_inner(custom_instructions).await;

        *self.phase.write().await = AgentHarnessPhase::Idle;
        result
    }

    async fn compact_inner(
        &self,
        custom_instructions: Option<&str>,
    ) -> Result<Option<CompactResult>, HarnessError> {
        let model = self.model.read().await.clone();
        let settings = crate::harness::types::DEFAULT_COMPACTION_SETTINGS;
        let entries = self.session.read().await.get_entries().await;

        let preparation = crate::harness::compaction::compaction::prepare_compaction(
            &entries,
            model.context_window,
            &settings,
        );

        match preparation {
            Ok(prep) => {
                // Emit session_before_compact hook
                let hook_payload = serde_json::json!({
                    "preparation": prep,
                });
                let _hook_result: Option<crate::harness::types::SessionBeforeCompactHookResult> =
                    self.emit_hook("session_before_compact", hook_payload)
                        .await
                        .and_then(|v| serde_json::from_value(v).ok());

                // Get API key
                let api_key = {
                    let get_auth = self.get_api_key_and_headers.read().await;
                    match get_auth.as_ref() {
                        Some(f) => f(&model)
                            .await
                            .map(|(k, _)| k)
                            .or_else(|| {
                                pi_ai::env_api_keys::get_env_api_key(&model.provider)
                            }),
                        None => pi_ai::env_api_keys::get_env_api_key(&model.provider),
                    }
                }
                .unwrap_or_default();

                let result = crate::harness::compaction::compaction::compact(
                    prep,
                    &model,
                    &api_key,
                    None,
                    custom_instructions,
                    None,
                    None,
                )
                .await
                .map_err(|e| HarnessError::Compaction(e))?;

                {
                    let mut session = self.session.write().await;
                    session
                        .append_compaction(
                            result.summary.clone(),
                            result.first_kept_entry_id.clone(),
                            result.tokens_before,
                            result.details.clone(),
                            Some(true),
                            None,
                        )
                        .await
                        .map_err(HarnessError::Session)?;
                }

                self.emit_own(AgentHarnessOwnEvent::SessionCompact {
                    result: result.clone(),
                    from_hook: false,
                })
                .await;

                Ok(Some(result))
            }
            Err(crate::harness::types::CompactionError::NoCompactionNeeded) => Ok(None),
            Err(e) => Err(HarnessError::Compaction(e)),
        }
    }

    // ========================================================
    // Public API: navigate_tree
    // ========================================================

    pub async fn navigate_tree(
        &self,
        target_id: &str,
        options: Option<NavigateTreeOptions>,
    ) -> Result<NavigateTreeResult, HarnessError> {
        if *self.phase.read().await != AgentHarnessPhase::Idle {
            return Err(HarnessError::Busy(
                "navigateTree() requires idle harness".into(),
            ));
        }

        *self.phase.write().await = AgentHarnessPhase::BranchSummary;

        let result = self
            .navigate_tree_inner(target_id, options)
            .await;

        *self.phase.write().await = AgentHarnessPhase::Idle;
        result
    }

    async fn navigate_tree_inner(
        &self,
        target_id: &str,
        options: Option<NavigateTreeOptions>,
    ) -> Result<NavigateTreeResult, HarnessError> {
        let opts = options.unwrap_or_default();

        let old_leaf_id = self.session.read().await.get_leaf_id().await;
        if old_leaf_id.as_deref() == Some(target_id) {
            return Ok(NavigateTreeResult {
                cancelled: false,
                editor_text: None,
                summary_entry: None,
            });
        }

        let target_entry = self.session.read().await.get_entry(target_id).await;
        if target_entry.is_none() {
            return Err(HarnessError::InvalidArgument(format!(
                "Entry {} not found",
                target_id
            )));
        }

        // Emit session_before_tree hook
        let hook_payload = serde_json::json!({
            "target_id": target_id,
            "old_leaf_id": old_leaf_id,
        });
        let _hook_result = self.emit_hook("session_before_tree", hook_payload).await;

        // Generate branch summary if requested
        let mut summary_entry: Option<BranchSummaryEntry> = None;
        let mut summary_text: Option<String> = None;
        let mut summary_details: Option<serde_json::Value> = None;

        if opts.summarize {
            let model = self.model.read().await.clone();
            let api_key = {
                let get_auth = self.get_api_key_and_headers.read().await;
                match get_auth.as_ref() {
                    Some(f) => f(&model).await.map(|(k, _)| k),
                    None => pi_ai::env_api_keys::get_env_api_key(&model.provider),
                }
            };

            if let Some(_api_key) = api_key {
                let branch_entries = self
                    .session
                    .read()
                    .await
                    .get_branch(Some(target_id))
                    .await
                    .map_err(HarnessError::Session)?;

                let branch_summary_options =
                    GenerateBranchSummaryOptions {
                        model: model.clone(),
                        reserve_tokens: None,
                        custom_instructions: opts.custom_instructions.clone(),
                        replace_instructions: Some(opts.replace_instructions),
                    };

                let branch_summary = crate::harness::compaction::branch_summarization::generate_branch_summary(
                    &branch_entries,
                    &branch_summary_options,
                )
                .await
                .map_err(|e| HarnessError::BranchSummary(e))?;

                summary_text = Some(branch_summary.summary.clone());
                summary_details = Some(serde_json::json!({
                    "readFiles": branch_summary.read_files,
                    "modifiedFiles": branch_summary.modified_files,
                }));
            }
        }

        let summary = summary_text.as_ref().map(|text| crate::harness::types::MoveToSummary {
            summary: text.clone(),
            details: summary_details,
            from_hook: None,
        });

        let summary_id = self
            .session
            .write()
            .await
            .move_to(Some(target_id), summary)
            .await
            .map_err(HarnessError::Session)?;

        if let Some(ref sid) = summary_id {
            if let Some(crate::harness::types::SessionTreeEntry::BranchSummary { .. }) =
                self.session.read().await.get_entry(sid).await
            {
                // We have a summary entry — build summary_entry
                summary_entry = Some(crate::harness::types::BranchSummaryEntry {
                    id: sid.clone(),
                    parent_id: None,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    from_id: target_id.to_string(),
                    summary: summary_text.clone().unwrap_or_default(),
                    details: None,
                    from_hook: None,
                });
            }
        }

        self.emit_own(AgentHarnessOwnEvent::SessionTree {
            new_leaf_id: self.session.read().await.get_leaf_id().await,
            old_leaf_id,
            summary_entry: summary_entry.clone(),
            from_hook: Some(false),
        })
        .await;

        Ok(NavigateTreeResult {
            cancelled: false,
            editor_text: None,
            summary_entry,
        })
    }
}

// ============================================================
// NavigateTreeOptions
// ============================================================

#[derive(Debug, Clone)]
pub struct NavigateTreeOptions {
    pub summarize: bool,
    pub custom_instructions: Option<String>,
    pub replace_instructions: bool,
}

impl Default for NavigateTreeOptions {
    fn default() -> Self {
        Self {
            summarize: false,
            custom_instructions: None,
            replace_instructions: false,
        }
    }
}

// ============================================================
// AgentHarnessOptions
// ============================================================

#[derive(Clone)]
pub struct AgentHarnessOptions<S: Clone = Skill, P: Clone = PromptTemplate> {
    pub thinking_level: Option<ThinkingLevel>,
    pub active_tool_names: Option<Vec<String>>,
    pub resources: Option<AgentHarnessResources<S, P>>,
    pub stream_options: Option<AgentHarnessStreamOptions>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
    pub tools: Option<Vec<String>>,
    pub system_prompt_factory: Option<SystemPromptProvider<S, P>>,
    pub get_api_key_and_headers: Option<GetApiKeyAndHeadersFn>,
}

impl<S: Clone, P: Clone> Default for AgentHarnessOptions<S, P> {
    fn default() -> Self {
        Self {
            thinking_level: None,
            active_tool_names: None,
            resources: None,
            stream_options: None,
            steering_mode: None,
            follow_up_mode: None,
            tools: None,
            system_prompt_factory: None,
            get_api_key_and_headers: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::types::{
        AgentHarnessStreamOptionsPatch, apply_stream_options_patch, find_duplicate_names,
    };
    use crate::pi_ai_types::StopReason;

    // --------------------------------------------------
    // Helper function tests
    // --------------------------------------------------

    #[test]
    fn test_create_user_message_basic() {
        let msg = create_user_message("hello", None);
        match msg {
            AgentMessage::User { content, .. } => {
                assert_eq!(content.len(), 1);
                assert_eq!(
                    content[0],
                    ContentBlock::Text {
                        text: "hello".into(),
                        text_signature: None,
                    }
                );
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn test_create_user_message_with_images() {
        let img = ContentBlock::Image {
            data: "data:image/png;base64,...".into(),
            mime_type: "image/png".into(),
        };
        let msg = create_user_message("look", Some(vec![img.clone()]));
        match msg {
            AgentMessage::User { content, .. } => {
                assert_eq!(content.len(), 2);
                assert_eq!(content[1], img);
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn test_create_failure_message_error() {
        let model = Model {
            id: "test-model".into(),
            name: "Test Model".into(),
            api: "test-api".into(),
            provider: "test-provider".into(),
            base_url: "https://test.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: crate::pi_ai_types::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            context_window: 100000,
            max_tokens: 4096,
            headers: None,
            compat: None,
        };
        let msg = create_failure_message(&model, "something went wrong", false);
        match msg {
            AgentMessage::Assistant {
                stop_reason, error_message, ..
            } => {
                assert_eq!(stop_reason, Some(StopReason::Error));
                assert_eq!(error_message, Some("something went wrong".into()));
            }
            _ => panic!("expected Assistant message"),
        }
    }

    #[test]
    fn test_create_failure_message_aborted() {
        let model = Model {
            id: "test-model".into(),
            name: "Test Model".into(),
            api: "test-api".into(),
            provider: "test-provider".into(),
            base_url: "https://test.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: crate::pi_ai_types::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            context_window: 100000,
            max_tokens: 4096,
            headers: None,
            compat: None,
        };
        let msg = create_failure_message(&model, "cancelled", true);
        match msg {
            AgentMessage::Assistant {
                stop_reason, error_message, ..
            } => {
                assert_eq!(stop_reason, Some(StopReason::Aborted));
                assert_eq!(error_message, Some("cancelled".into()));
            }
            _ => panic!("expected Assistant message"),
        }
    }

    #[test]
    fn test_merge_headers_empty() {
        assert_eq!(merge_headers(&[]), None);
        assert_eq!(merge_headers(&[None, None]), None);
    }

    #[test]
    fn test_merge_headers_single() {
        let h = Some(HashMap::from([("key1".into(), "val1".into())]));
        let result = merge_headers(&[h]);
        assert_eq!(result, Some(HashMap::from([("key1".into(), "val1".into())])));
    }

    #[test]
    fn test_merge_headers_multiple() {
        let h1 = Some(HashMap::from([("key1".into(), "val1".into())]));
        let h2 = Some(HashMap::from([("key2".into(), "val2".into())]));
        let result = merge_headers(&[h1, h2]);
        let expected = HashMap::from([
            ("key1".into(), "val1".into()),
            ("key2".into(), "val2".into()),
        ]);
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn test_merge_headers_later_overwrites() {
        let h1 = Some(HashMap::from([("key".into(), "old".into())]));
        let h2 = Some(HashMap::from([("key".into(), "new".into())]));
        let result = merge_headers(&[h1, h2]);
        assert_eq!(result, Some(HashMap::from([("key".into(), "new".into())])));
    }

    #[test]
    fn test_find_duplicate_names_none() {
        let names = vec!["a".into(), "b".into(), "c".into()];
        assert!(find_duplicate_names(&names).is_empty());
    }

    #[test]
    fn test_find_duplicate_names_some() {
        let names = vec!["a".into(), "b".into(), "a".into(), "c".into(), "b".into()];
        let dups = find_duplicate_names(&names);
        assert_eq!(dups.len(), 2);
        assert!(dups.contains(&"a".into()));
        assert!(dups.contains(&"b".into()));
    }

    #[test]
    fn test_find_duplicate_names_empty() {
        assert!(find_duplicate_names(&[]).is_empty());
    }

    #[test]
    fn test_clone_stream_options() {
        let opts = AgentHarnessStreamOptions {
            temperature: Some(0.7),
            top_p: Some(0.9),
            max_tokens: Some(2048),
            transport: Some("websocket".into()),
            timeout_ms: Some(30000),
            max_retries: Some(3),
            max_retry_delay_ms: Some(1000),
            cache_retention: Some("short".into()),
            headers: Some(HashMap::from([("X-Custom".into(), "val".into())])),
            metadata: Some(HashMap::from([("key".into(), "value".into())])),
        };
        let cloned = clone_stream_options(&opts);
        // Verify all fields match
        assert_eq!(cloned.temperature, opts.temperature);
        assert_eq!(cloned.top_p, opts.top_p);
        assert_eq!(cloned.max_tokens, opts.max_tokens);
        assert_eq!(cloned.transport, opts.transport);
        assert_eq!(cloned.timeout_ms, opts.timeout_ms);
        assert_eq!(cloned.max_retries, opts.max_retries);
        assert_eq!(cloned.max_retry_delay_ms, opts.max_retry_delay_ms);
        assert_eq!(cloned.cache_retention, opts.cache_retention);
        assert_eq!(cloned.headers, opts.headers);
        assert_eq!(cloned.metadata, opts.metadata);
    }

    #[test]
    fn test_apply_stream_options_patch() {
        let base = AgentHarnessStreamOptions {
            temperature: Some(0.5),
            top_p: None,
            max_tokens: Some(1000),
            transport: None,
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: None,
            metadata: None,
        };
        let patch = AgentHarnessStreamOptionsPatch {
            temperature: Some(Some(0.9)),
            top_p: None,
            max_tokens: Some(None), // explicitly clear
            transport: Some(Some("sse".into())),
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: None,
            metadata: None,
        };
        let result = apply_stream_options_patch(&base, &patch);
        assert_eq!(result.temperature, Some(0.9));
        assert_eq!(result.top_p, None); // unchanged
        assert_eq!(result.max_tokens, None); // cleared
        assert_eq!(result.transport, Some("sse".into())); // set
    }

    // --------------------------------------------------
    // QueueMode tests
    // --------------------------------------------------

    #[test]
    fn test_queue_mode_variants() {
        assert_ne!(QueueMode::Queue as u8, QueueMode::Replace as u8);
        assert_ne!(QueueMode::Replace as u8, QueueMode::Drop as u8);
        assert_ne!(QueueMode::Queue as u8, QueueMode::Drop as u8);
    }

    // --------------------------------------------------
    // AgentHarnessPhase tests
    // --------------------------------------------------

    #[test]
    fn test_harness_phase_default_is_idle() {
        // Idle should be the first variant (default-safe)
        let phase = AgentHarnessPhase::Idle;
        assert_eq!(phase as u8, 0);
    }

    #[test]
    fn test_harness_phase_transition_order() {
        let idle = AgentHarnessPhase::Idle;
        let turn = AgentHarnessPhase::Turn;
        let compaction = AgentHarnessPhase::Compaction;
        let branch = AgentHarnessPhase::BranchSummary;

        assert_ne!(idle as u8, turn as u8);
        assert_ne!(turn as u8, compaction as u8);
        assert_ne!(compaction as u8, branch as u8);
    }

    #[test]
    fn test_harness_phase_debug_clone() {
        let phase = AgentHarnessPhase::Turn;
        let cloned = phase.clone();
        assert_eq!(format!("{:?}", phase), format!("{:?}", cloned));
    }

    // --------------------------------------------------
    // PendingSessionWrite tests
    // --------------------------------------------------

    #[test]
    fn test_pending_session_write_message() {
        let msg = create_user_message("test", None);
        let write = PendingSessionWrite::Message { message: msg.clone() };
        match write {
            PendingSessionWrite::Message { message } => {
                assert_eq!(
                    format!("{:?}", message),
                    format!("{:?}", msg)
                );
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_pending_session_write_model_change() {
        let write = PendingSessionWrite::ModelChange {
            provider: "anthropic".into(),
            model_id: "claude-3".into(),
        };
        match write {
            PendingSessionWrite::ModelChange { provider, model_id } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(model_id, "claude-3");
            }
            _ => panic!("expected ModelChange variant"),
        }
    }

    // --------------------------------------------------
    // Named trait tests
    // --------------------------------------------------

    #[test]
    fn test_named_skill() {
        let skill = Skill {
            name: "test-skill".into(),
            description: "A test skill".into(),
            content: "Do something".into(),
            file_path: String::new(),
            disable_model_invocation: false,
        };
        assert_eq!(skill.name(), "test-skill");
    }

    #[test]
    fn test_named_prompt_template() {
        let tmpl = PromptTemplate {
            name: "test-template".into(),
            description: String::new(),
            content: "Hello {name}".into(),
        };
        assert_eq!(tmpl.name(), "test-template");
    }

    // --------------------------------------------------
    // AgentHarnessEvent tests
    // --------------------------------------------------

    #[test]
    fn test_harness_event_agent_variant() {
        let agent_event = AgentEvent::AgentStart;
        let event: AgentHarnessEvent<Skill, PromptTemplate> =
            AgentHarnessEvent::Agent(agent_event.clone());
        match event {
            AgentHarnessEvent::Agent(AgentEvent::AgentStart) => {}
            _ => panic!("expected Agent::AgentStart variant"),
        }
    }

    #[test]
    fn test_harness_event_own_variant() {
        let own_event = AgentHarnessOwnEvent::<Skill, PromptTemplate>::Abort {
            cleared_steer: vec![],
            cleared_follow_up: vec![],
        };
        let event = AgentHarnessEvent::Own(own_event.clone());
        match event {
            AgentHarnessEvent::Own(e) => {
                assert_eq!(format!("{:?}", e), format!("{:?}", own_event));
            }
            _ => panic!("expected Own variant"),
        }
    }

    // --------------------------------------------------
    // HarnessError tests
    // --------------------------------------------------

    #[test]
    fn test_harness_error_busy() {
        let err = HarnessError::Busy("harness is busy".into());
        assert_eq!(format!("{}", err), "busy: harness is busy");
    }

    #[test]
    fn test_harness_error_invalid_argument() {
        let err = HarnessError::InvalidArgument("bad arg".into());
        assert_eq!(format!("{}", err), "invalid_argument: bad arg");
    }

    #[test]
    fn test_harness_error_session() {
        let inner = crate::harness::types::SessionError::Storage("db error".into());
        let err = HarnessError::Session(inner);
        assert!(format!("{}", err).contains("db error"));
    }
}
