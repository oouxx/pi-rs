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

pub struct AgentOptions {
    pub initial_state: Option<AgentState>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    pub transform_context: Option<TransformContextFn>,
    pub stream_fn: Option<StreamFn>,
    pub get_api_key: Option<GetApiKeyFn>,
    pub on_payload: Option<Arc<dyn Fn(serde_json::Value) + Send + Sync>>,
    pub on_response: Option<Arc<dyn Fn(&AssistantMessage) + Send + Sync>>,
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

pub struct Agent {
    state: Arc<RwLock<AgentState>>,
    listeners: Arc<RwLock<Vec<AgentEventListener>>>,
    active_run: Arc<Mutex<Option<ActiveRun>>>,
    convert_to_llm: ConvertToLlmFn,
    transform_context: Option<TransformContextFn>,
    stream_fn: StreamFn,
    get_api_key: Option<GetApiKeyFn>,
    on_payload: Option<Arc<dyn Fn(serde_json::Value) + Send + Sync>>,
    on_response: Option<Arc<dyn Fn(&AssistantMessage) + Send + Sync>>,
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
                        AgentEvent::TurnEnd { message, .. } => {
                            if let AgentMessage::Assistant {
                                error_message: Some(err),
                                ..
                            } = message
                            {
                                s.error_message = Some(err.clone());
                            }
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
