use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{watch, Mutex, Notify, RwLock};

use crate::pi_ai_types::{
    AssistantMessage, ContentBlock, Model, ModelCost, StopReason, ThinkingLevel, Usage,
};
use crate::types::{
    AfterToolCallFn, AgentContext, AgentEvent, AgentEventSink, AgentMessage, AgentState,
    BeforeToolCallFn, ConvertToLlmFn, GetApiKeyFn, PrepareNextTurnFn, PrepareNextTurnOptionsFn,
    QueueMode, ShouldStopAfterTurnFn, StreamFn, TransformContextFn,
};

/// Input to `Agent::prompt()`. Matches TS `Agent.prompt()` overloads.
pub enum PromptInput<'a> {
    /// A batch of messages.
    Messages(Vec<AgentMessage>),
    /// A single text string.
    Text(&'a str),
    /// A text string with images.
    TextWithImages {
        text: &'a str,
        images: Vec<ImageContentRef<'a>>,
    },
}

/// Reference to image content for prompt input.
pub struct ImageContentRef<'a> {
    pub data: &'a str,
    pub mime_type: &'a str,
}

struct PendingMessageQueue {
    messages: Vec<AgentMessage>,
    mode: QueueMode,
}

impl PendingMessageQueue {
    fn new(mode: QueueMode) -> Self {
        Self {
            messages: Vec::new(),
            mode,
        }
    }

    fn enqueue(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    fn drain(&mut self) -> Vec<AgentMessage> {
        match self.mode {
            QueueMode::All => std::mem::take(&mut self.messages),
            QueueMode::OneAtATime => {
                if self.messages.is_empty() {
                    Vec::new()
                } else {
                    vec![self.messages.remove(0)]
                }
            }
        }
    }

    fn has_items(&self) -> bool {
        !self.messages.is_empty()
    }

    fn clear(&mut self) {
        self.messages.clear();
    }
}

type AgentEventListener = Arc<
    dyn Fn(
            AgentEvent,
            Option<watch::Receiver<bool>>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

#[allow(clippy::type_complexity)]
pub struct AgentOptions {
    pub initial_state: Option<AgentState>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    pub transform_context: Option<TransformContextFn>,
    pub stream_fn: Option<StreamFn>,
    pub get_api_key: Option<GetApiKeyFn>,
    pub on_payload: Option<Arc<dyn Fn(serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send>> + Send + Sync>>,
    pub on_response: Option<Arc<dyn Fn(&AssistantMessage) + Send + Sync>>,
    pub on_headers: Option<Arc<dyn Fn(std::collections::HashMap<String, String>) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::collections::HashMap<String, String>> + Send>> + Send + Sync>>,
    pub on_provider_response: Option<Arc<dyn Fn(u16, std::collections::HashMap<String, String>) + Send + Sync>>,
    pub before_tool_call: Option<BeforeToolCallFn>,
    pub after_tool_call: Option<AfterToolCallFn>,
    /// Takes an optional abort signal (no turn context). Matches TS `AgentOptions.prepareNextTurn`.
    pub prepare_next_turn: Option<PrepareNextTurnOptionsFn>,
    /// Takes the full turn context. Matches TS `AgentOptions.prepareNextTurnWithContext`.
    pub prepare_next_turn_with_context: Option<PrepareNextTurnFn>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
    pub session_id: Option<String>,
    pub thinking_budgets: Option<crate::pi_ai_types::ThinkingBudgets>,
    pub transport: Option<String>,
    pub max_retry_delay_ms: Option<u64>,
    pub tool_execution: Option<crate::pi_ai_types::ToolExecutionMode>,
    pub max_consecutive_tool_calls: Option<usize>,
}

#[allow(clippy::derivable_impls)]
impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            initial_state: None,
            convert_to_llm: None,
            transform_context: None,
            stream_fn: None,
            get_api_key: None,
            on_payload: None,
            on_response: None,
            on_headers: None,
            on_provider_response: None,
            before_tool_call: None,
            after_tool_call: None,
            prepare_next_turn: None,
            prepare_next_turn_with_context: None,
            steering_mode: None,
            follow_up_mode: None,
            session_id: None,
            thinking_budgets: None,
            transport: None,
            max_retry_delay_ms: None,
            tool_execution: None,
            max_consecutive_tool_calls: None,
        }
    }
}

struct ActiveRun {
    cancel: tokio_util::sync::CancellationToken,
}

/// Handle returned by [`Agent::subscribe`]. Call `unsubscribe()` to stop
/// receiving events. Dropping the handle does NOT unsubscribe — the listener
/// remains registered, matching the original TypeScript behavior where dropping
/// the unsubscribe function does not remove the listener.
pub struct UnsubscribeHandle {
    listeners: Arc<RwLock<Vec<AgentEventListener>>>,
    index: usize,
}

impl UnsubscribeHandle {
    /// Remove the listener from the agent. After this call the listener will
    /// no longer receive events.
    pub async fn unsubscribe(self) {
        let mut listeners = self.listeners.write().await;
        if self.index < listeners.len() {
            // Replace with a no-op so the slot stays valid and Vec indices
            // are not disturbed.
            listeners[self.index] = Arc::new(|_, _| Box::pin(async {}));
        }
    }
}

#[allow(clippy::type_complexity)]
#[derive(Clone)]
pub struct Agent {
    state: Arc<RwLock<AgentState>>,
    listeners: Arc<RwLock<Vec<AgentEventListener>>>,
    active_run: Arc<Mutex<Option<ActiveRun>>>,
    convert_to_llm: ConvertToLlmFn,
    transform_context: Option<TransformContextFn>,
    stream_fn: StreamFn,
    get_api_key: Option<GetApiKeyFn>,
    on_payload: Option<Arc<dyn Fn(serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send>> + Send + Sync>>,
    on_response: Option<Arc<dyn Fn(&AssistantMessage) + Send + Sync>>,
    on_headers: Option<Arc<dyn Fn(std::collections::HashMap<String, String>) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::collections::HashMap<String, String>> + Send>> + Send + Sync>>,
    on_provider_response: Option<Arc<dyn Fn(u16, std::collections::HashMap<String, String>) + Send + Sync>>,
    before_tool_call: Option<BeforeToolCallFn>,
    after_tool_call: Option<AfterToolCallFn>,
    prepare_next_turn: Option<PrepareNextTurnOptionsFn>,
    prepare_next_turn_with_context: Option<PrepareNextTurnFn>,
    should_stop_after_turn: Option<ShouldStopAfterTurnFn>,
    steering_queue: Arc<Mutex<PendingMessageQueue>>,
    follow_up_queue: Arc<Mutex<PendingMessageQueue>>,
    session_id: Option<String>,
    thinking_budgets: Option<crate::pi_ai_types::ThinkingBudgets>,
    transport: String,
    max_retry_delay_ms: Option<u64>,
    tool_execution: crate::pi_ai_types::ToolExecutionMode,
    max_consecutive_tool_calls: Option<usize>,
    /// Notified when the agent becomes idle (finishes a run or streaming ends).
    idle_notify: Arc<Notify>,
}

fn default_convert_to_llm(messages: &[AgentMessage]) -> Vec<crate::pi_ai_types::Message> {
    crate::harness::messages::convert_to_llm(messages)
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            model: Model {
                provider: String::new(),
                api: String::new(),
                id: String::new(),
                name: String::new(),
                base_url: String::new(),
                context_window: 0,
                max_tokens: 0,
                cost: crate::pi_ai_types::ModelCost::default(),
                reasoning: false,
                thinking_level_map: None,
                input: vec![],
                headers: None,
                compat: None,
            },
            thinking_level: crate::pi_ai_types::THINKING_OFF.to_string(),
            tools: Vec::new(),
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: HashSet::new(),
            error_message: None,
        }
    }
}

impl Agent {
    pub fn new(options: AgentOptions) -> Self {
        let state = options.initial_state.unwrap_or_else(|| AgentState {
            system_prompt: String::new(),
            model: Model {
                provider: String::new(),
                api: String::new(),
                id: String::new(),
                name: String::new(),
                base_url: String::new(),
                context_window: 0,
                max_tokens: 0,
                cost: ModelCost::default(),
                reasoning: false,
                thinking_level_map: None,
                input: vec![],
                headers: None,
                compat: None,
            },
            thinking_level: "off".to_string(),
            tools: Vec::new(),
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: HashSet::new(),
            error_message: None,
        });

        let convert_to_llm = options
            .convert_to_llm
            .unwrap_or_else(|| Arc::new(default_convert_to_llm));

        let stream_fn = options.stream_fn.unwrap_or_else(|| {
            Arc::new(|_model, _ctx, _thinking, _opts| {
                Box::pin(async {
                    Err::<crate::pi_ai_types::StreamResponse, _>(
                        "No stream function configured".into(),
                    )
                })
            })
        });

        Self {
            state: Arc::new(RwLock::new(state)),
            listeners: Arc::new(RwLock::new(Vec::new())),
            active_run: Arc::new(Mutex::new(None)),
            convert_to_llm,
            transform_context: options.transform_context,
            stream_fn,
            get_api_key: options.get_api_key,
            on_payload: options.on_payload,
            on_response: options.on_response,
            on_headers: options.on_headers,
            on_provider_response: options.on_provider_response,
            before_tool_call: options.before_tool_call,
            after_tool_call: options.after_tool_call,
            prepare_next_turn: options.prepare_next_turn,
            prepare_next_turn_with_context: options.prepare_next_turn_with_context,
            should_stop_after_turn: None,
            steering_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                options.steering_mode.unwrap_or(QueueMode::OneAtATime),
            ))),
            follow_up_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                options.follow_up_mode.unwrap_or(QueueMode::OneAtATime),
            ))),
            session_id: options.session_id,
            thinking_budgets: options.thinking_budgets,
            transport: options.transport.unwrap_or_else(|| "auto".to_string()),
            max_retry_delay_ms: options.max_retry_delay_ms,
            tool_execution: options
                .tool_execution
                .unwrap_or(crate::pi_ai_types::ToolExecutionMode::Parallel),
            max_consecutive_tool_calls: options.max_consecutive_tool_calls,
            idle_notify: Arc::new(Notify::new()),
        }
    }

    pub fn set_should_stop_after_turn(&mut self, f: ShouldStopAfterTurnFn) {
        self.should_stop_after_turn = Some(f);
    }

    /// Subscribe to agent events.
    ///
    /// Returns an [`UnsubscribeHandle`] – call `handle.unsubscribe().await`
    /// to stop receiving events, or drop it for best-effort cleanup.
    pub async fn subscribe(&self, listener: AgentEventListener) -> UnsubscribeHandle {
        let mut listeners = self.listeners.write().await;
        listeners.push(listener);
        UnsubscribeHandle {
            listeners: self.listeners.clone(),
            index: listeners.len() - 1,
        }
    }

    pub async fn state(&self) -> AgentState {
        self.state.read().await.clone()
    }

    /// Add or replace tools in the agent's tool list.
    ///
    /// Tools with the same `name` as an existing entry are replaced in-place,
    /// allowing callers to upgrade stub tool entries (created from
    /// `custom_tools` metadata) with real execute implementations.
    pub async fn add_tools(&self, tools: Vec<Arc<crate::types::DynTool>>) {
        let mut state = self.state.write().await;
        for tool in tools {
            if let Some(pos) = state.tools.iter().position(|t| t.name == tool.name) {
                state.tools[pos] = tool;
            } else {
                state.tools.push(tool);
            }
        }
    }

    pub async fn set_steering_mode(&self, mode: QueueMode) {
        self.steering_queue.lock().await.mode = mode;
    }

    pub async fn steering_mode(&self) -> QueueMode {
        self.steering_queue.lock().await.mode
    }

    pub async fn set_follow_up_mode(&self, mode: QueueMode) {
        self.follow_up_queue.lock().await.mode = mode;
    }

    pub async fn follow_up_mode(&self) -> QueueMode {
        self.follow_up_queue.lock().await.mode
    }

    pub async fn steer(&self, message: AgentMessage) {
        self.steering_queue.lock().await.enqueue(message);
    }

    pub async fn follow_up(&self, message: AgentMessage) {
        self.follow_up_queue.lock().await.enqueue(message);
    }

    pub async fn clear_steering_queue(&self) {
        self.steering_queue.lock().await.clear();
    }

    pub async fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().await.clear();
    }

    pub async fn clear_all_queues(&self) {
        self.clear_steering_queue().await;
        self.clear_follow_up_queue().await;
    }

    pub async fn has_queued_messages(&self) -> bool {
        self.steering_queue.lock().await.has_items()
            || self.follow_up_queue.lock().await.has_items()
    }

    pub async fn abort(&self) {
        if let Some(run) = self.active_run.lock().await.as_ref() {
            run.cancel.cancel();
        }
    }

    /// Active cancellation token for the current run, if any.
    pub async fn cancellation_token(&self) -> Option<tokio_util::sync::CancellationToken> {
        self.active_run
            .lock()
            .await
            .as_ref()
            .map(|r| r.cancel.clone())
    }

    /// Active abort signal for the current run, if any.
    /// Matches TS `get signal(): AbortSignal | undefined`.
    pub fn get_stream_fn(&self) -> Option<StreamFn> {
        Some(self.stream_fn.clone())
    }

    pub fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
        self.active_run
            .try_lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|r| r.cancel.clone()))
    }

    /// Wait until the agent is no longer streaming (idle).
    /// Uses an event-driven `Notify` — no polling.
    pub async fn wait_for_idle(&self) {
        let notified = self.idle_notify.notified();
        tokio::pin!(notified);
        loop {
            {
                let state = self.state.read().await;
                if !state.is_streaming {
                    return;
                }
            }
            notified.as_mut().await;
            notified.as_mut().enable();
        }
    }

    /// Reset the agent to its initial state, clearing messages and aborting any active run.
    pub async fn reset(&self) {
        self.abort().await;
        self.clear_all_queues().await;
        let mut state = self.state.write().await;
        state.messages.clear();
        state.is_streaming = false;
        state.streaming_message = None;
        state.pending_tool_calls.clear();
        state.error_message = None;
    }

    /// Process messages through the agent loop.
    /// Deprecated: use `prompt()` instead.
    pub async fn process(
        &self,
        messages: Vec<AgentMessage>,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        self.prompt(PromptInput::Messages(messages)).await
    }

    /// Start a new prompt from text, a single message, or a batch of messages.
    /// Matches TS `Agent.prompt()` with overloads.
    pub async fn prompt(
        &self,
        input: PromptInput<'_>,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        {
            let active = self.active_run.lock().await;
            if active.is_some() {
                return Err("Agent is already processing a prompt. Use steer() or follow_up() to queue messages, or wait for completion.".into());
            }
        }

        let messages = self.normalize_prompt_input(input);
        self.run_prompt_messages(messages, false).await
    }

    /// Continue from the current transcript. The last message must be a user or tool-result message.
    /// Matches TS `Agent.continue()`.
    pub async fn continue_run(
        &self,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        {
            let active = self.active_run.lock().await;
            if active.is_some() {
                return Err(
                    "Agent is already processing. Wait for completion before continuing.".into(),
                );
            }
        }

        {
            let state = self.state.read().await;
            if state.messages.is_empty() {
                return Err("Cannot continue: no messages in context".into());
            }

            // TS behavior: if last message is assistant, drain steering/follow-up first
            if state.messages.last().map(|m| m.role()) == Some("assistant") {
                drop(state);
                {
                    let mut state = self.state.write().await;
                    state.is_streaming = true;
                    state.streaming_message = None;
                    state.error_message = None;
                }

                // Try steering first
                let steering_msgs = self.steering_queue.lock().await.drain();
                if !steering_msgs.is_empty() {
                    let result = self.run_prompt_messages(steering_msgs, true).await;
                    self.finish_run().await;
                    return result;
                }

                // Try follow-up next
                let follow_up_msgs = self.follow_up_queue.lock().await.drain();
                if !follow_up_msgs.is_empty() {
                    let result = self.run_prompt_messages(follow_up_msgs, false).await;
                    self.finish_run().await;
                    return result;
                }

                {
                    let mut state = self.state.write().await;
                    state.is_streaming = false;
                }
                return Err("Cannot continue from message role: assistant".into());
            }
        }
        {
            let mut state = self.state.write().await;
            state.is_streaming = true;
            state.streaming_message = None;
            state.error_message = None;
        }

        let result = self.run_continuation().await;
        self.finish_run().await;
        result
    }

    /// Normalize prompt input to a Vec<AgentMessage>.
    /// Matches TS `normalizePromptInput()`.
    fn normalize_prompt_input(&self, input: PromptInput<'_>) -> Vec<AgentMessage> {
        match input {
            PromptInput::Messages(msgs) => msgs,
            PromptInput::Text(text) => {
                vec![AgentMessage::User {
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                        text_signature: None,
                    }],
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }]
            }
            PromptInput::TextWithImages { text, images } => {
                let mut content: Vec<ContentBlock> = vec![ContentBlock::Text {
                    text: text.to_string(),
                    text_signature: None,
                }];
                for img in images {
                    content.push(ContentBlock::Image {
                        data: img.data.to_string(),
                        mime_type: img.mime_type.to_string(),
                    });
                }
                vec![AgentMessage::User {
                    content,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }]
            }
        }
    }

    /// Run prompt messages through the agent loop.
    /// Matches TS `runPromptMessages()`.
    async fn run_prompt_messages(
        &self,
        messages: Vec<AgentMessage>,
        skip_initial_steering_poll: bool,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        {
            let mut state = self.state.write().await;
            state.is_streaming = true;
            state.streaming_message = None;
            state.error_message = None;
        }

        let result = self.run_with_lifecycle(messages, skip_initial_steering_poll).await;
        self.finish_run().await;
        result
    }

    /// Run a continuation (retry) from the current context.
    /// Matches TS `runContinuation()`.
    async fn run_continuation(
        &self,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        {
            let mut state = self.state.write().await;
            state.is_streaming = true;
            state.streaming_message = None;
            state.error_message = None;
        }

        let result = self.run_with_lifecycle_continue().await;
        self.finish_run().await;
        result
    }

    /// Create the agent loop config from current state.
    /// Matches TS `createLoopConfig()`.
    async fn create_loop_config(
        &self,
        cancel_rx: watch::Receiver<bool>,
        skip_initial_steering_poll: bool,
    ) -> (crate::agent_loop::AgentLoopConfig, Option<watch::Receiver<bool>>) {
        let steering_queue = self.steering_queue.clone();
        let follow_up_queue = self.follow_up_queue.clone();
        let skip_steering = Arc::new(tokio::sync::Mutex::new(skip_initial_steering_poll));

        let state = self.state.read().await;
        let config = crate::agent_loop::AgentLoopConfig {
            model: state.model.clone(),
            reasoning: {
                let tl = state.thinking_level.clone();
                if tl == "off" { None } else { Some(tl) }
            },
            api_key: None,
            session_id: self.session_id.clone(),
            thinking_budgets: self.thinking_budgets.clone(),
            transport: Some(self.transport.clone()),
            max_retry_delay_ms: self.max_retry_delay_ms,
            tool_execution: self.tool_execution,
            convert_to_llm: self.convert_to_llm.clone(),
            transform_context: self.transform_context.clone(),
            get_api_key: self.get_api_key.clone(),
            get_steering_messages: Some(Arc::new({
                let skip = skip_steering.clone();
                let q = steering_queue.clone();
                move || {
                    let skip = skip.clone();
                    let q = q.clone();
                    Box::pin(async move {
                        let mut guard = skip.lock().await;
                        if *guard {
                            *guard = false;
                            return Vec::new();
                        }
                        drop(guard);
                        q.lock().await.drain()
                    })
                }
            })),
            get_follow_up_messages: Some(Arc::new(move || {
                let q = follow_up_queue.clone();
                Box::pin(async move { q.lock().await.drain() })
            })),
            should_stop_after_turn: self.should_stop_after_turn.clone(),
            prepare_next_turn: {
                let pnt = self.prepare_next_turn.clone();
                let pntwc = self.prepare_next_turn_with_context.clone();
                let cancel_rx_clone = cancel_rx.clone();
                if pntwc.is_some() || pnt.is_some() {
                    Some(Arc::new(move |ctx: crate::types::ShouldStopAfterTurnContext, sig: Option<tokio::sync::watch::Receiver<bool>>| {
                        let pnt = pnt.clone();
                        let pntwc = pntwc.clone();
                        let sig = sig.or_else(|| Some(cancel_rx_clone.clone()));
                        Box::pin(async move {
                            if let Some(f) = pntwc {
                                f(ctx, sig).await
                            } else if let Some(f) = pnt {
                                f(sig).await
                            } else {
                                None
                            }
                        })
                    }))
                } else {
                    None
                }
            },
            before_tool_call: self.before_tool_call.clone(),
            after_tool_call: self.after_tool_call.clone(),
            on_payload: self.on_payload.clone(),
            on_response: self.on_response.clone(),
            on_headers: self.on_headers.clone(),
            on_provider_response: self.on_provider_response.clone(),
            max_consecutive_tool_calls: self.max_consecutive_tool_calls,
        };

        let signal = Some(cancel_rx);
        (config, signal)
    }

    async fn finish_run(&self) {
        {
            let mut state = self.state.write().await;
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
        }
        self.idle_notify.notify_waiters();
    }

    async fn run_with_lifecycle(
        &self,
        prompts: Vec<AgentMessage>,
        skip_initial_steering_poll: bool,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            cancel.cancelled().await;
            let _ = cancel_tx.send(true);
        });

        let active_run = ActiveRun {
            cancel: cancel_clone,
        };
        *self.active_run.lock().await = Some(active_run);

        let state = self.state.read().await;
        let context = AgentContext {
            system_prompt: state.system_prompt.clone(),
            messages: state.messages.clone(),
            tools: Some(state.tools.clone()),
        };
        let model = state.model.clone();
        drop(state);

        let emit = self.create_event_sink();
        let (config, signal) = self.create_loop_config(cancel_rx, skip_initial_steering_poll).await;

        let loop_result = crate::agent_loop::run_agent_loop(
            prompts,
            context,
            &config,
            &emit,
            &signal,
            &self.stream_fn,
        )
        .await;

        self.active_run.lock().await.take();

        let was_aborted = signal.as_ref().map(|rx| *rx.borrow()).unwrap_or(false);

        match loop_result {
            Ok(messages) => Ok(messages),
            Err(e) => {
                self.handle_run_failure(e, was_aborted, &model, &emit).await;
                Ok(vec![]) // handle_run_failure emits the failure message
            }
        }
    }

    async fn run_with_lifecycle_continue(
        &self,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            cancel.cancelled().await;
            let _ = cancel_tx.send(true);
        });

        let active_run = ActiveRun {
            cancel: cancel_clone,
        };
        *self.active_run.lock().await = Some(active_run);

        let state = self.state.read().await;
        let context = AgentContext {
            system_prompt: state.system_prompt.clone(),
            messages: state.messages.clone(),
            tools: Some(state.tools.clone()),
        };
        let _model = state.model.clone();
        drop(state);

        let emit = self.create_event_sink();
        let (config, signal) = self.create_loop_config(cancel_rx, false).await;

        let result = crate::agent_loop::run_agent_loop_continue(
            context,
            &config,
            &emit,
            &signal,
            &self.stream_fn,
        )
        .await;

        self.active_run.lock().await.take();

        result
    }

    /// Handle a run failure by synthesizing a failure message and emitting lifecycle events.
    /// Matches TS `handleRunFailure()`.
    async fn handle_run_failure(
        &self,
        error: Box<dyn std::error::Error + Send + Sync>,
        aborted: bool,
        model: &Model,
        emit: &AgentEventSink,
    ) {
        let failure_message = AgentMessage::Assistant {
            content: vec![ContentBlock::Text {
                text: String::new(),
                text_signature: None,
            }],
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            usage: Usage::default(),
            stop_reason: Some(if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            }),
            error_message: Some(error.to_string()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        emit(AgentEvent::MessageStart {
            message: failure_message.clone(),
        })
        .await;
        emit(AgentEvent::MessageEnd {
            message: failure_message.clone(),
        })
        .await;
        emit(AgentEvent::TurnEnd {
            message: failure_message.clone(),
            tool_results: Vec::new(),
        })
        .await;
        emit(AgentEvent::AgentEnd {
            messages: vec![failure_message],
        })
        .await;
    }

    fn create_event_sink(&self) -> AgentEventSink {
        let listeners = self.listeners.clone();
        let state = self.state.clone();
        let idle_notify = self.idle_notify.clone();

        Arc::new(move |event: AgentEvent| {
            let listeners = listeners.clone();
            let state = state.clone();
            let idle_notify = idle_notify.clone();
            Box::pin(async move {
                // Reduce internal state (matches TS processEvents)
                {
                    let mut s = state.write().await;
                    match &event {
                        AgentEvent::MessageStart { message } => {
                            if matches!(message, AgentMessage::Assistant { .. }) {
                                s.streaming_message = Some(message.clone());
                            }
                        }
                        AgentEvent::MessageUpdate { message, .. } => {
                            s.streaming_message = Some(message.clone());
                        }
                        AgentEvent::MessageEnd { message } => {
                            s.streaming_message = None;
                            s.messages.push(message.clone());
                        }
                        AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
                            s.pending_tool_calls.insert(tool_call_id.clone());
                        }
                        AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
                            s.pending_tool_calls.remove(tool_call_id);
                        }
                        AgentEvent::TurnEnd {
                            message: AgentMessage::Assistant {
                                error_message: Some(err),
                                ..
                            },
                            ..
                        } => {
                            s.error_message = Some(err.clone());
                        }
                        AgentEvent::AgentEnd { .. } => {
                            s.is_streaming = false;
                            s.streaming_message = None;
                        }
                        _ => {}
                    }
                }

                // Notify idle waiters on AgentEnd (after state update, before listeners)
                if matches!(&event, AgentEvent::AgentEnd { .. }) {
                    idle_notify.notify_waiters();
                }

                // Await all listeners (matches TS listener loop)
                let listeners_guard = listeners.read().await;
                for listener in listeners_guard.iter() {
                    listener(event.clone(), None).await;
                }
            })
        })
    }

    pub async fn set_model(&self, model: Model) {
        self.state.write().await.model = model;
    }

    /// Set initial messages (e.g., loaded from a session file) before
    /// any processing begins. Replaces whatever is in the state.
    pub async fn set_initial_messages(&self, messages: Vec<AgentMessage>) {
        self.state.write().await.messages = messages;
    }

    pub async fn set_thinking_level(&self, level: ThinkingLevel) {
        self.state.write().await.thinking_level = level;
    }

    pub async fn set_system_prompt(&self, prompt: String) {
        self.state.write().await.system_prompt = prompt;
    }

    pub async fn messages(&self) -> Vec<AgentMessage> {
        self.state.read().await.messages.clone()
    }

    pub async fn is_streaming(&self) -> bool {
        self.state.read().await.is_streaming
    }

    pub async fn error_message(&self) -> Option<String> {
        self.state.read().await.error_message.clone()
    }
}

/// Convenience builder that reduces boilerplate for common Agent setups.
///
/// All optional fields use the same defaults as [`AgentOptions`] / [`Agent::new`].
pub fn create_agent(
    model: Model,
    system_prompt: impl Into<String>,
    tools: Vec<Arc<crate::types::DynTool>>,
    stream_fn: StreamFn,
    convert_to_llm: ConvertToLlmFn,
) -> Agent {
    Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: system_prompt.into(),
            model,
            tools,
            ..Default::default()
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(convert_to_llm),
        ..Default::default()
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(unnameable_test_items, clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};


    use crate::pi_ai_types::{
        AssistantMessage, AssistantMessageEvent, ContentBlock, Model, ModelCost, StopReason,
        Usage,
    };
    use crate::types::{AgentEvent, AgentMessage, AgentState, QueueMode, StreamFn, StreamFnOptions};

    use super::*;

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn make_model() -> Model {
        Model {
            id: "test-model".into(),
            name: "test".into(),
            api: "test-api".into(),
            provider: "test-provider".into(),
            base_url: "https://test.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            context_window: 100_000,
            max_tokens: 4096,
            headers: None,
            compat: None,
        }
    }

    fn make_assistant_message(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![ContentBlock::text(text)],
            api: "test-api".into(),
            provider: "test-provider".into(),
            model: "test-model".into(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            response_model: None,
            response_id: None,
            diagnostics: None,
            error_message: None,
            timestamp: 1000,
        }
    }

    /// Create a stream function that emits a single "done" event with the given text.
    fn make_ok_stream_fn(text: &str) -> StreamFn {
        let msg = make_assistant_message(text);
        Arc::new(move |_model, _context, _thinking, _opts: StreamFnOptions| {
            let msg = msg.clone();
            Box::pin(async move {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let _ = tx.send(AssistantMessageEvent::Done {
                    message: msg.clone(),
                    reason: StopReason::Stop,
                });
                let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
            })
        })
    }

    /// Create a stream function that fails with an error.
    fn make_failing_stream_fn(err_msg: &str) -> StreamFn {
        let err_msg = err_msg.to_string();
        Arc::new(move |_model, _context, _thinking, _opts: StreamFnOptions| {
            let err_msg = err_msg.clone();
            Box::pin(async move {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let _ = tx.send(AssistantMessageEvent::Error {
                    error: AssistantMessage {
                        content: vec![ContentBlock::text(&err_msg)],
                        api: "test-api".into(),
                        provider: "test-provider".into(),
                        model: "test-model".into(),
                        response_model: None,
                        response_id: None,
                        diagnostics: None,
                        usage: Usage::default(),
                        stop_reason: StopReason::Error,
                        error_message: Some(err_msg.clone()),
                        timestamp: 1000,
                    },
                    reason: StopReason::Error,
                });
                let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
            })
        })
    }

    fn default_convert_to_llm() -> ConvertToLlmFn {
        Arc::new(|_msgs: &[AgentMessage]| vec![])
    }

    // ------------------------------------------------------------------
    // Tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_default_state() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("hello")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        let state = agent.state().await;
        assert_eq!(state.system_prompt, "");
        assert!(state.model.id.is_empty() || state.model.id == "test-model");
        assert_eq!(state.thinking_level, "off");
        assert!(state.tools.is_empty());
        assert!(state.messages.is_empty());
        assert!(!state.is_streaming);
        assert!(state.error_message.is_none());
    }

    #[tokio::test]
    async fn test_agent_custom_initial_state() {
        let custom_model = make_model();
        let agent = Agent::new(AgentOptions {
            initial_state: Some(AgentState {
                system_prompt: "You are a helpful assistant.".into(),
                model: custom_model.clone(),
                thinking_level: "low".to_string(),
                ..Default::default()
            }),
            stream_fn: Some(make_ok_stream_fn("hello")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        let state = agent.state().await;
        assert_eq!(state.system_prompt, "You are a helpful assistant.");
        assert_eq!(state.model.id, custom_model.id);
        assert_eq!(state.thinking_level, "low");
    }

    #[tokio::test]
    async fn test_agent_subscribe_events() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("hello")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        let event_count = Arc::new(AtomicUsize::new(0));
        let count_clone = event_count.clone();
        let _unsub = agent.subscribe(Arc::new(move |_event, _signal| {
            let c = count_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
        }))
        .await;

        // No events yet
        assert_eq!(event_count.load(Ordering::SeqCst), 0);

        // Run a prompt
        #[allow(clippy::unwrap_used)]
        agent.prompt(PromptInput::Text("hello")).await.unwrap();

        // Should have received events
        assert!(
            event_count.load(Ordering::SeqCst) > 0,
            "Expected at least 1 event, got {}",
            event_count.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn test_agent_lifecycle_events_failure() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_failing_stream_fn("provider exploded")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        let events = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let ev_clone = events.clone();
        agent
            .subscribe(Arc::new(move |event, _signal| {
                let ev = ev_clone.clone();
                Box::pin(async move {
                    let event_type = match &event {
                        AgentEvent::AgentStart => "agent_start",
                        AgentEvent::TurnStart => "turn_start",
                        AgentEvent::MessageStart { .. } => "message_start",
                        AgentEvent::MessageEnd { .. } => "message_end",
                        AgentEvent::TurnEnd { .. } => "turn_end",
                        AgentEvent::AgentEnd { .. } => "agent_end",
                        _ => "other",
                    };
                    ev.lock().await.push(event_type.to_string());
                })
            }))
            .await;

        #[allow(clippy::unwrap_used)]
        agent.prompt(PromptInput::Text("hello")).await.unwrap();

        let events = events.lock().await;
        assert_eq!(
            *events,
            vec![
                "agent_start",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_end",
                "turn_end",
                "agent_end",
            ],
            "Expected lifecycle events, got: {:?}",
            *events
        );

        let state = agent.state().await;
        let last_msg = state.messages.last().unwrap();
        assert_eq!(last_msg.role(), "assistant");
        assert_eq!(state.error_message.as_deref(), Some("provider exploded"));
    }

    #[tokio::test]
    async fn test_agent_async_subscribers() {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let b1 = barrier.clone();

        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("ok")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        let listener_finished = Arc::new(AtomicUsize::new(0));
        let lf = listener_finished.clone();
        agent
            .subscribe(Arc::new(move |event, _signal| {
                let b = b1.clone();
                let l = lf.clone();
                Box::pin(async move {
                    if matches!(event, AgentEvent::AgentEnd { .. }) {
                        // Wait for the barrier
                        b.wait().await;
                        l.store(1, Ordering::SeqCst);
                    }
                })
            }))
            .await;

        // Start prompt in background
        let agent_clone = agent.clone();
        let prompt_handle = tokio::spawn(async move {
            agent_clone.prompt(PromptInput::Text("hello")).await.unwrap();
        });

        // Give the prompt time to start and reach AgentEnd
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Listener should not have finished yet (waiting on barrier)
        assert_eq!(listener_finished.load(Ordering::SeqCst), 0);

        // Release the barrier
        barrier.wait().await;

        // Now listener should have finished
        prompt_handle.await.unwrap();
        assert_eq!(listener_finished.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_agent_wait_for_idle() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("ok")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Start a prompt
        let agent_clone = agent.clone();
        let handle = tokio::spawn(async move {
            agent_clone.prompt(PromptInput::Text("hello")).await.unwrap();
        });

        // wait_for_idle should complete after the prompt finishes
        agent.wait_for_idle().await;
        handle.await.unwrap();

        let state = agent.state().await;
        assert!(!state.is_streaming);
    }

    #[tokio::test]
    async fn test_agent_abort_signal() {
        let saw_abort = Arc::new(AtomicUsize::new(0));
        let sa = saw_abort.clone();

        let sa_for_subscribe = sa.clone();
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(Arc::new(move |_model, _context, _thinking, _opts: StreamFnOptions| {
                Box::pin(async move {
                    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    // Never send done - just hang
                    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                    Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
                })
            })),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        agent
            .subscribe(Arc::new(move |_event, signal| {
                let sa = sa_for_subscribe.clone();
                Box::pin(async move {
                    if let Some(sig) = signal {
                        let mut rx = sig.clone();
                        tokio::spawn(async move {
                            rx.changed().await.ok();
                            if *rx.borrow() {
                                sa.store(1, Ordering::SeqCst);
                            }
                        });
                    }
                })
            }))
            .await;

        // Start a prompt that hangs
        let agent_clone = agent.clone();
        let handle = tokio::spawn(async move {
            let _ = agent_clone.prompt(PromptInput::Text("hello")).await;
        });

        // Give it time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Abort
        agent.abort().await;

        handle.await.ok();
    }

    #[tokio::test]
    async fn test_agent_state_mutators() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("ok")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Test set_system_prompt
        agent.set_system_prompt("Test prompt".into()).await;
        assert_eq!(agent.state().await.system_prompt, "Test prompt");

        // Test set_thinking_level
        agent.set_thinking_level("high".to_string()).await;
        assert_eq!(agent.state().await.thinking_level, "high");

        // Test set_model
        let new_model = make_model();
        agent.set_model(new_model.clone()).await;
        assert_eq!(agent.state().await.model.id, new_model.id);

        // Test messages
        assert!(agent.messages().await.is_empty());

        // Test is_streaming
        assert!(!agent.is_streaming().await);

        // Test error_message
        assert!(agent.error_message().await.is_none());
    }

    #[tokio::test]
    async fn test_agent_steering_queue() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();

        let agent = Agent::new(AgentOptions {
            stream_fn: Some(Arc::new(move |_model, _context, _thinking, _opts: StreamFnOptions| {
                cc.fetch_add(1, Ordering::SeqCst);
                let msg = make_assistant_message("ok");
                Box::pin(async move {
                    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    let _ = _tx.send(AssistantMessageEvent::Done {
                        message: msg,
                        reason: StopReason::Stop,
                    });
                    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                    Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
                })
            })),
            convert_to_llm: Some(default_convert_to_llm()),
            steering_mode: Some(QueueMode::OneAtATime),
            ..Default::default()
        });

        // Set initial messages so continue() works
        agent
            .set_initial_messages(vec![
                AgentMessage::User {
                    content: vec![ContentBlock::text("Initial")],
                    timestamp: 1000,
                },
                AgentMessage::Assistant {
                    content: vec![ContentBlock::text("Initial response")],
                    api: "test".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    usage: Usage::default(),
                    stop_reason: Some(StopReason::Stop),
                    error_message: None,
                    timestamp: 1000,
                },
            ])
            .await;

        // Steer two messages
        agent
            .steer(AgentMessage::User {
                content: vec![ContentBlock::text("Steering 1")],
                timestamp: 1000,
            })
            .await;
        agent
            .steer(AgentMessage::User {
                content: vec![ContentBlock::text("Steering 2")],
                timestamp: 1001,
            })
            .await;

        // Continue should process all messages (one-at-a-time within the loop)
        agent.continue_run().await.unwrap();

        // All messages should be processed
        assert!(!agent.has_queued_messages().await);
    }

    #[tokio::test]
    async fn test_agent_follow_up_queue() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("ok")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Set initial messages
        agent
            .set_initial_messages(vec![
                AgentMessage::User {
                    content: vec![ContentBlock::text("Initial")],
                    timestamp: 1000,
                },
                AgentMessage::Assistant {
                    content: vec![ContentBlock::text("Initial response")],
                    api: "test".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    usage: Usage::default(),
                    stop_reason: Some(StopReason::Stop),
                    error_message: None,
                    timestamp: 1000,
                },
            ])
            .await;

        // Follow up
        agent
            .follow_up(AgentMessage::User {
                content: vec![ContentBlock::text("Queued follow-up")],
                timestamp: 1000,
            })
            .await;

        assert!(agent.has_queued_messages().await);

        // Continue should process the follow-up
        agent.continue_run().await.unwrap();

        let messages = agent.messages().await;
        let has_follow_up = messages.iter().any(|m| match m {
            AgentMessage::User { content, .. } => {
                content.iter().any(|c| matches!(c, ContentBlock::Text { text, .. } if text == "Queued follow-up"))
            }
            _ => false,
        });
        assert!(has_follow_up, "Follow-up message should be in context");
    }

    #[tokio::test]
    async fn test_agent_abort_controller() {
        // abort() should not throw when nothing is running
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("ok")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });
        agent.abort().await;

        // Verify abort works during streaming
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(Arc::new(|_model, _context, _thinking, opts: StreamFnOptions| {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AssistantMessageEvent>();
                // Send start event immediately so streaming begins
                let start_msg = AssistantMessage {
                    content: vec![ContentBlock::text("")],
                    api: "test".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 1000,
                };
                let _ = tx.send(AssistantMessageEvent::Start { partial: start_msg });
                // Watch the abort signal in a spawned task
                if let Some(mut sig) = opts.signal.clone() {
                    tokio::spawn(async move {
                        let _ = sig.changed().await;
                        if *sig.borrow() {
                            let _ = tx.send(AssistantMessageEvent::Error {
                                error: AssistantMessage {
                                    content: vec![ContentBlock::text("Aborted")],
                                    api: "test".into(),
                                    provider: "test".into(),
                                    model: "test".into(),
                                    response_model: None,
                                    response_id: None,
                                    diagnostics: None,
                                    usage: Usage::default(),
                                    stop_reason: StopReason::Aborted,
                                    error_message: Some("Aborted".into()),
                                    timestamp: 1000,
                                },
                                reason: StopReason::Aborted,
                            });
                        }
                    });
                }
                let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                Box::pin(async move {
                    Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
                })
            })),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Start a prompt that will be aborted
        let agent_clone = agent.clone();
        let handle = tokio::spawn(async move {
            let _ = agent_clone.prompt(PromptInput::Text("hello")).await;
        });

        // Wait for streaming to start
        for _ in 0..100 {
            if agent.is_streaming().await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(agent.is_streaming().await);

        // Abort
        agent.abort().await;

        // Wait for prompt to complete (with timeout)
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await.unwrap_or(Ok(()));

        assert!(!agent.is_streaming().await);
    }

    #[tokio::test]
    async fn test_agent_throw_when_streaming() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(Arc::new(|_model, _context, _thinking, _opts: StreamFnOptions| {
                Box::pin(async move {
                    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    // Never send done - just hang
                    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                    Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
                })
            })),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Start first prompt
        let agent_clone = agent.clone();
        let _first = tokio::spawn(async move {
            let _ = agent_clone.prompt(PromptInput::Text("First message")).await;
        });

        // Give it time to start streaming
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // continue_run() should fail when already streaming
        let result = agent.continue_run().await;
        assert!(
            result.is_err(),
            "continue_run() should fail when already streaming"
        );

        // Cleanup
        agent.abort().await;
    }

    #[tokio::test]
    async fn test_agent_continue_follow_up() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("Processed")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Set initial messages
        agent
            .set_initial_messages(vec![
                AgentMessage::User {
                    content: vec![ContentBlock::text("Initial")],
                    timestamp: 1000,
                },
                AgentMessage::Assistant {
                    content: vec![ContentBlock::text("Initial response")],
                    api: "test".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    usage: Usage::default(),
                    stop_reason: Some(StopReason::Stop),
                    error_message: None,
                    timestamp: 1000,
                },
            ])
            .await;

        // Follow up
        agent
            .follow_up(AgentMessage::User {
                content: vec![ContentBlock::text("Queued follow-up")],
                timestamp: 1000,
            })
            .await;

        // Continue
        agent.continue_run().await.unwrap();

        let messages = agent.messages().await;
        let has_follow_up = messages.iter().any(|m| match m {
            AgentMessage::User { content, .. } => {
                content.iter().any(|c| matches!(c, ContentBlock::Text { text, .. } if text == "Queued follow-up"))
            }
            _ => false,
        });
        assert!(has_follow_up, "Follow-up message should be in context");

        // Last message should be assistant
        let last = messages.last().unwrap();
        assert_eq!(last.role(), "assistant");
    }

    #[tokio::test]
    async fn test_agent_one_at_a_time_steering() {
        let response_count = Arc::new(AtomicUsize::new(0));
        let rc = response_count.clone();

        let agent = Agent::new(AgentOptions {
            stream_fn: Some(Arc::new(move |_model, _context, _thinking, _opts: StreamFnOptions| {
                let count = rc.fetch_add(1, Ordering::SeqCst) + 1;
                let msg = make_assistant_message(&format!("Processed {}", count));
                Box::pin(async move {
                    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    let _ = _tx.send(AssistantMessageEvent::Done {
                        message: msg,
                        reason: StopReason::Stop,
                    });
                    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                    Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
                })
            })),
            convert_to_llm: Some(default_convert_to_llm()),
            steering_mode: Some(QueueMode::OneAtATime),
            ..Default::default()
        });

        // Set initial messages
        agent
            .set_initial_messages(vec![
                AgentMessage::User {
                    content: vec![ContentBlock::text("Initial")],
                    timestamp: 1000,
                },
                AgentMessage::Assistant {
                    content: vec![ContentBlock::text("Initial response")],
                    api: "test".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    usage: Usage::default(),
                    stop_reason: Some(StopReason::Stop),
                    error_message: None,
                    timestamp: 1000,
                },
            ])
            .await;

        // Steer two messages
        agent
            .steer(AgentMessage::User {
                content: vec![ContentBlock::text("Steering 1")],
                timestamp: 1000,
            })
            .await;
        agent
            .steer(AgentMessage::User {
                content: vec![ContentBlock::text("Steering 2")],
                timestamp: 1001,
            })
            .await;

        // Continue - should process both messages (one-at-a-time within the loop)
        agent.continue_run().await.unwrap();

        let messages = agent.messages().await;
        let recent: Vec<&str> = messages.iter().rev().take(4).map(|m| m.role()).collect();
        assert_eq!(recent, vec!["assistant", "user", "assistant", "user"]);

        assert_eq!(response_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_agent_session_id() {
        let received_id = Arc::new(tokio::sync::Mutex::new(None::<String>));
        let rid = received_id.clone();

        let agent = Agent::new(AgentOptions {
            session_id: Some("session-abc".into()),
            stream_fn: Some(Arc::new(move |_model, _context, _thinking, opts: StreamFnOptions| {
                let rid = rid.clone();
                Box::pin(async move {
                    if let Some(sid) = &opts.session_id {
                        *rid.lock().await = Some(sid.to_string());
                    }
                    let msg = make_assistant_message("ok");
                    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    let _ = _tx.send(AssistantMessageEvent::Done {
                        message: msg,
                        reason: StopReason::Stop,
                    });
                    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                    Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
                })
            })),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        #[allow(clippy::unwrap_used)]
        agent.prompt(PromptInput::Text("hello")).await.unwrap();

        let sid = received_id.lock().await.clone();
        assert_eq!(sid.as_deref(), Some("session-abc"));
    }

    #[tokio::test]
    async fn test_agent_reset() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("ok")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Run a prompt
        #[allow(clippy::unwrap_used)]
        agent.prompt(PromptInput::Text("hello")).await.unwrap();

        let state = agent.state().await;
        assert!(!state.messages.is_empty());

        // Reset
        agent.reset().await;

        let state = agent.state().await;
        assert!(state.messages.is_empty());
        assert!(!state.is_streaming);
        assert!(state.error_message.is_none());
    }

    #[tokio::test]
    async fn test_agent_clear_queues() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(make_ok_stream_fn("ok")),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Add messages to both queues
        agent
            .steer(AgentMessage::User {
                content: vec![ContentBlock::text("Steer msg")],
                timestamp: 1000,
            })
            .await;
        agent
            .follow_up(AgentMessage::User {
                content: vec![ContentBlock::text("Follow-up msg")],
                timestamp: 1000,
            })
            .await;

        assert!(agent.has_queued_messages().await);

        // Clear all queues
        agent.clear_all_queues().await;

        assert!(!agent.has_queued_messages().await);
    }

    #[tokio::test]
    async fn test_agent_throw_when_continue_streaming() {
        let agent = Agent::new(AgentOptions {
            stream_fn: Some(Arc::new(|_model, _context, _thinking, opts: StreamFnOptions| {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AssistantMessageEvent>();
                // Send start event immediately so streaming begins
                let start_msg = AssistantMessage {
                    content: vec![ContentBlock::text("")],
                    api: "test".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 1000,
                };
                let _ = tx.send(AssistantMessageEvent::Start { partial: start_msg });
                // Watch the abort signal in a spawned task
                if let Some(mut sig) = opts.signal.clone() {
                    tokio::spawn(async move {
                        let _ = sig.changed().await;
                        if *sig.borrow() {
                            let _ = tx.send(AssistantMessageEvent::Error {
                                error: AssistantMessage {
                                    content: vec![ContentBlock::text("Aborted")],
                                    api: "test".into(),
                                    provider: "test".into(),
                                    model: "test".into(),
                                    response_model: None,
                                    response_id: None,
                                    diagnostics: None,
                                    usage: Usage::default(),
                                    stop_reason: StopReason::Aborted,
                                    error_message: Some("Aborted".into()),
                                    timestamp: 1000,
                                },
                                reason: StopReason::Aborted,
                            });
                        }
                    });
                }
                let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                Box::pin(async move {
                    Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
                })
            })),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        // Start a prompt (don't await - it will block until abort)
        let agent_clone = agent.clone();
        let handle = tokio::spawn(async move {
            let _ = agent_clone.prompt(PromptInput::Text("First message")).await;
        });

        // Wait for streaming to start
        for _ in 0..100 {
            if agent.is_streaming().await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(agent.is_streaming().await);

        // continue_run() should fail when already streaming
        let result = agent.continue_run().await;
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("already processing") || err_msg.contains("already"),
            "Expected error about already processing, got: {}",
            err_msg
        );

        // Cleanup - abort to stop the stream
        agent.abort().await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await.unwrap_or(Ok(()));
    }

    #[tokio::test]
    async fn test_agent_prepare_next_turn_signal() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let rc = request_count.clone();
        let saw_signal = Arc::new(AtomicUsize::new(0));
        let ss = saw_signal.clone();

        let agent = Agent::new(AgentOptions {
            stream_fn: Some(Arc::new(move |_model, _context, _thinking, _opts: StreamFnOptions| {
                let count = rc.fetch_add(1, Ordering::SeqCst) + 1;
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AssistantMessageEvent>();
                if count == 1 {
                    // First call: return a tool use message
                    let msg = AssistantMessage {
                        content: vec![ContentBlock::ToolCall {
                            id: "tool-1".into(),
                            name: "noop".into(),
                            arguments: serde_json::json!({}),
                            thought_signature: None,
                        }],
                        api: "test".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        response_model: None,
                        response_id: None,
                        diagnostics: None,
                        usage: Usage::default(),
                        stop_reason: StopReason::ToolUse,
                        error_message: None,
                        timestamp: 1000,
                    };
                    let _ = tx.send(AssistantMessageEvent::Done {
                        message: msg,
                        reason: StopReason::ToolUse,
                    });
                } else {
                    // Second call: return a stop message
                    let msg = AssistantMessage {
                        content: vec![ContentBlock::text("done")],
                        api: "test".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        response_model: None,
                        response_id: None,
                        diagnostics: None,
                        usage: Usage::default(),
                        stop_reason: StopReason::Stop,
                        error_message: None,
                        timestamp: 1000,
                    };
                    let _ = tx.send(AssistantMessageEvent::Done {
                        message: msg,
                        reason: StopReason::Stop,
                    });
                }
                let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                Box::pin(async move {
                    Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
                })
            })),
            convert_to_llm: Some(default_convert_to_llm()),
            prepare_next_turn: Some(Arc::new(move |_signal: Option<tokio::sync::watch::Receiver<bool>>| {
                let ss = ss.clone();
                Box::pin(async move {
                    ss.store(1, Ordering::SeqCst);
                    None
                })
            })),
            ..Default::default()
        });

        // Register a noop tool so the tool call can be executed
        use crate::types::AgentTool;
        let noop_tool = AgentTool {
            name: "noop".into(),
            label: "Noop".into(),
            description: "A noop tool".into(),
            parameters_schema: serde_json::json!({}),
            execution_mode: None,
            prepare_arguments: None,
            execute: Arc::new(|_tool_call_id: String, _params: serde_json::Value, _signal: Option<tokio::sync::watch::Receiver<bool>>, _on_update: Option<crate::types::AgentToolUpdateCallback<serde_json::Value>>| {
                Box::pin(async move {
                    Ok(crate::types::AgentToolResult {
                        content: vec![ContentBlock::text("ok")],
                        details: serde_json::json!({}),
                        terminate: None,
                    })
                })
            }),
        };
        agent.add_tools(vec![Arc::new(noop_tool)]).await;

        agent.prompt(PromptInput::Text("start")).await.unwrap();

        assert_eq!(request_count.load(Ordering::SeqCst), 2, "Should have made 2 stream calls");
        assert_eq!(saw_signal.load(Ordering::SeqCst), 1, "prepare_next_turn should have been called");
    }

    // TS: "should ignore tool updates after the tool execution settles"
    // Verifies that after a tool's execute function returns, late calls to
    // onUpdate are ignored.
    #[tokio::test]
    async fn test_agent_ignore_tool_updates_after_settle() {
        let delayed_update = Arc::new(tokio::sync::Mutex::new(None::<crate::types::AgentToolUpdateCallback<serde_json::Value>>));
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));

        let du = delayed_update.clone();

        // Create a tool that captures the onUpdate callback
        use crate::types::AgentTool;
        let delayed_tool = AgentTool {
            name: "delayed_tool".into(),
            label: "Delayed Tool".into(),
            description: "Captures progress callbacks".into(),
            parameters_schema: serde_json::json!({}),
            execution_mode: None,
            prepare_arguments: None,
            execute: Arc::new(move |_tool_call_id: String, _params: serde_json::Value, _signal: Option<tokio::sync::watch::Receiver<bool>>, on_update: Option<crate::types::AgentToolUpdateCallback<serde_json::Value>>| {
                let du = du.clone();
                Box::pin(async move {
                    // Store the on_update callback
                    if let Some(cb) = on_update {
                        let mut guard = du.lock().await;
                        *guard = Some(cb.clone());
                        // Call on_update during execution
                        cb(crate::types::AgentToolResult {
                            content: vec![ContentBlock::text("running")],
                            details: serde_json::json!({"status": "running"}),
                            terminate: None,
                        });
                    }
                    // Return immediately
                    Ok(crate::types::AgentToolResult {
                        content: vec![ContentBlock::text("ok")],
                        details: serde_json::json!({"status": "done"}),
                        terminate: Some(true),
                    })
                })
            }),
        };

        // Create a stream that returns a tool_use message
        let stream_fn: StreamFn = Arc::new(|_model, _context, _thinking, _opts: StreamFnOptions| {
            Box::pin(async move {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let msg = AssistantMessage {
                    content: vec![ContentBlock::ToolCall {
                        id: "call-1".into(),
                        name: "delayed_tool".into(),
                        arguments: serde_json::json!({}),
                        thought_signature: None,
                    }],
                    api: "test".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::ToolUse,
                    error_message: None,
                    timestamp: 1000,
                };
                let _ = tx.send(AssistantMessageEvent::Done {
                    message: msg,
                    reason: StopReason::ToolUse,
                });
                let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
            })
        });

        let agent = Agent::new(AgentOptions {
            stream_fn: Some(stream_fn),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        let ev2 = events.clone();
        agent
            .subscribe(Arc::new(move |event, _signal| {
                let ev = ev2.clone();
                Box::pin(async move {
                    ev.lock().unwrap().push(event.clone());
                })
            }))
            .await;

        agent.add_tools(vec![Arc::new(delayed_tool)]).await;

        agent.prompt(PromptInput::Text("run tool")).await.unwrap();

        let event_count_after_prompt = events.lock().unwrap().len();

        // Now call on_update after the tool has settled
        {
            let guard = delayed_update.lock().await;
            if let Some(cb) = guard.as_ref() {
                cb(crate::types::AgentToolResult {
                    content: vec![ContentBlock::text("late")],
                    details: serde_json::json!({"status": "late"}),
                    terminate: None,
                });
            }
        }

        // Give time for the late update to be processed
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let final_events = events.lock().unwrap();
        let update_count = final_events.iter().filter(|e| matches!(e, AgentEvent::ToolExecutionUpdate { .. })).count();
        assert_eq!(update_count, 1, "should have exactly 1 tool_execution_update");
        assert_eq!(final_events.len(), event_count_after_prompt, "no new events after settle");
    }

    // TS: "should ignore a settled parallel tool update while another tool is still running"
    // Verifies that when one tool finishes but another is still running, late
    // updates from the finished tool are ignored.
    #[tokio::test]
    async fn test_agent_ignore_parallel_tool_update_while_another_running() {
        let settled_update = Arc::new(tokio::sync::Mutex::new(None::<crate::types::AgentToolUpdateCallback<serde_json::Value>>));
        let slow_release = Arc::new(tokio::sync::Notify::new());
        let slow_started = Arc::new(tokio::sync::Notify::new());
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));

        let su = settled_update.clone();
        let _ev = events.clone();

        // Settled tool: finishes quickly, captures onUpdate
        use crate::types::AgentTool;
        let settled_tool = AgentTool {
            name: "settled_tool".into(),
            label: "Settled Tool".into(),
            description: "Finishes quickly".into(),
            parameters_schema: serde_json::json!({}),
            execution_mode: None,
            prepare_arguments: None,
            execute: Arc::new(move |_tool_call_id: String, _params: serde_json::Value, _signal: Option<tokio::sync::watch::Receiver<bool>>, on_update: Option<crate::types::AgentToolUpdateCallback<serde_json::Value>>| {
                let su = su.clone();
                Box::pin(async move {
                    if let Some(cb) = on_update {
                        let mut guard = su.lock().await;
                        *guard = Some(cb);
                    }
                    Ok(crate::types::AgentToolResult {
                        content: vec![ContentBlock::text("done")],
                        details: serde_json::json!({"status": "done"}),
                        terminate: Some(true),
                    })
                })
            }),
        };

        let slow_release_for_closure = slow_release.clone();
        // Slow tool: hangs until released
        let slow_tool = AgentTool {
            name: "slow_tool".into(),
            label: "Slow Tool".into(),
            description: "Hangs".into(),
            parameters_schema: serde_json::json!({}),
            execution_mode: None,
            prepare_arguments: None,
            execute: Arc::new(move |_tool_call_id: String, _params: serde_json::Value, _signal: Option<tokio::sync::watch::Receiver<bool>>, _on_update: Option<crate::types::AgentToolUpdateCallback<serde_json::Value>>| {
                let ss = slow_started.clone();
                let sr = slow_release_for_closure.clone();
                Box::pin(async move {
                    ss.notify_one();
                    sr.notified().await;
                    Ok(crate::types::AgentToolResult {
                        content: vec![ContentBlock::text("done")],
                        details: serde_json::json!({"status": "done"}),
                        terminate: Some(true),
                    })
                })
            }),
        };

        // Create a stream that returns both tool calls
        let stream_fn: StreamFn = Arc::new(|_model, _context, _thinking, _opts: StreamFnOptions| {
            Box::pin(async move {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let msg = AssistantMessage {
                    content: vec![
                        ContentBlock::ToolCall {
                            id: "call-1".into(),
                            name: "settled_tool".into(),
                            arguments: serde_json::json!({}),
                            thought_signature: None,
                        },
                        ContentBlock::ToolCall {
                            id: "call-2".into(),
                            name: "slow_tool".into(),
                            arguments: serde_json::json!({}),
                            thought_signature: None,
                        },
                    ],
                    api: "test".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::ToolUse,
                    error_message: None,
                    timestamp: 1000,
                };
                let _ = tx.send(AssistantMessageEvent::Done {
                    message: msg,
                    reason: StopReason::ToolUse,
                });
                let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                Ok(Box::new(stream) as crate::pi_ai_types::StreamResponse)
            })
        });

        let agent = Agent::new(AgentOptions {
            stream_fn: Some(stream_fn),
            convert_to_llm: Some(default_convert_to_llm()),
            ..Default::default()
        });

        let ev2 = events.clone();
        let settled_ended = Arc::new(tokio::sync::Notify::new());
        let se2 = settled_ended.clone();
        agent
            .subscribe(Arc::new(move |event, _signal| {
                let ev = ev2.clone();
                let se = se2.clone();
                Box::pin(async move {
                    ev.lock().unwrap().push(event.clone());
                    if matches!(event, AgentEvent::ToolExecutionEnd { tool_call_id, .. } if tool_call_id == "call-1") {
                        se.notify_one();
                    }
                })
            }))
            .await;

        agent.add_tools(vec![Arc::new(settled_tool), Arc::new(slow_tool)]).await;

        // Start prompt in background
        let agent_clone = agent.clone();
        let prompt_handle = tokio::spawn(async move {
            agent_clone.prompt(PromptInput::Text("run tools")).await.unwrap();
        });

        // Wait for slow tool to start and settled tool to end
        slow_started.notified().await;
        settled_ended.notified().await;

        let event_count_before_late = events.lock().unwrap().len();

        // Call on_update on the settled tool (should be ignored)
        {
            let guard = settled_update.lock().await;
            if let Some(cb) = guard.as_ref() {
                cb(crate::types::AgentToolResult {
                    content: vec![ContentBlock::text("late")],
                    details: serde_json::json!({"status": "late"}),
                    terminate: None,
                });
            }
        }

        // Give time for the late update to be processed
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // No new events should have been added
        {
            let final_events = events.lock().unwrap();
            assert_eq!(final_events.len(), event_count_before_late, "no new events after settled tool's late update");
        }

        // Release the slow tool
        slow_release_for_closure.notify_one();

        // Wait for prompt to complete
        prompt_handle.await.unwrap();

        // Final check: no tool_execution_update events at all
        let final_events = events.lock().unwrap();
        let update_count = final_events.iter().filter(|e| matches!(e, AgentEvent::ToolExecutionUpdate { .. })).count();
        assert_eq!(update_count, 0, "should have 0 tool_execution_update events");
    }
}
