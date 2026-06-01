use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{watch, Mutex, Notify, RwLock};

use crate::pi_ai_types::{AssistantMessage, ContentBlock, Model, ModelCost, StopReason, ThinkingLevel, Usage};
use crate::types::{
    AfterToolCallFn, AgentContext, AgentEvent, AgentEventSink, AgentMessage, AgentState,
    BeforeToolCallFn, ConvertToLlmFn, GetApiKeyFn, PrepareNextTurnFn, QueueMode,
    ShouldStopAfterTurnFn, StreamFn, TransformContextFn,
};

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
    dyn Fn(AgentEvent, Option<watch::Receiver<bool>>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
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
    pub prepare_next_turn: Option<PrepareNextTurnFn>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
    pub session_id: Option<String>,
    pub thinking_budgets: Option<crate::pi_ai_types::ThinkingBudgets>,
    pub transport: Option<String>,
    pub max_retry_delay_ms: Option<u64>,
    pub tool_execution: Option<crate::pi_ai_types::ToolExecutionMode>,
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
            steering_mode: None,
            follow_up_mode: None,
            session_id: None,
            thinking_budgets: None,
            transport: None,
            max_retry_delay_ms: None,
            tool_execution: None,
        }
    }
}

struct ActiveRun {
    cancel: tokio_util::sync::CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

/// Handle returned by [`Agent::subscribe`]. Call `unsubscribe()` to stop
/// receiving events, or drop it for best-effort cleanup.
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

impl std::ops::Drop for UnsubscribeHandle {
    fn drop(&mut self) {
        // Best-effort cleanup when the handle is dropped without calling
        // `unsubscribe()` explicitly.
        if let Ok(mut listeners) = self.listeners.try_write() {
            if self.index < listeners.len() {
                listeners[self.index] = Arc::new(|_, _| Box::pin(async {}));
            }
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
    prepare_next_turn: Option<PrepareNextTurnFn>,
    should_stop_after_turn: Option<ShouldStopAfterTurnFn>,
    steering_queue: Arc<Mutex<PendingMessageQueue>>,
    follow_up_queue: Arc<Mutex<PendingMessageQueue>>,
    session_id: Option<String>,
    thinking_budgets: Option<crate::pi_ai_types::ThinkingBudgets>,
    transport: String,
    max_retry_delay_ms: Option<u64>,
    tool_execution: crate::pi_ai_types::ToolExecutionMode,
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
        self.steering_queue.lock().await.has_items() || self.follow_up_queue.lock().await.has_items()
    }

    pub async fn abort(&self) {
        if let Some(run) = self.active_run.lock().await.as_ref() {
            run.cancel.cancel();
        }
    }

    /// Active cancellation token for the current run, if any.
    pub async fn cancellation_token(&self) -> Option<tokio_util::sync::CancellationToken> {
        self.active_run.lock().await.as_ref().map(|r| r.cancel.clone())
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

    pub async fn process(
        &self,
        messages: Vec<AgentMessage>,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        {
            let active = self.active_run.lock().await;
            if active.is_some() {
                return Err("Agent is already processing a prompt. Use steer() or follow_up() to queue messages, or wait for completion.".into());
            }
        }

        {
            let mut state = self.state.write().await;
            state.is_streaming = true;
            state.streaming_message = None;
            state.error_message = None;
        }

        let result = self.run_with_lifecycle(messages).await;
        self.finish_run().await;
        result
    }

    pub async fn continue_run(
        &self,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        {
            let active = self.active_run.lock().await;
            if active.is_some() {
                return Err("Agent is already processing. Wait for completion before continuing.".into());
            }
        }

        {
            let state = self.state.read().await;
            if state.messages.is_empty() {
                return Err("Cannot continue: no messages in context".into());
            }
            if state.messages.last().map(|m| m.role()) == Some("assistant") {
                return Err("Cannot continue from message role: assistant".into());
            }
        }
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
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            cancel.cancelled().await;
            let _ = cancel_tx.send(true);
        });

        let active_run = ActiveRun {
            cancel: cancel_clone,
            handle,
        };
        *self.active_run.lock().await = Some(active_run);

        let state = self.state.read().await;
        let context = AgentContext {
            system_prompt: state.system_prompt.clone(),
            messages: state.messages.clone(),
            tools: Some(state.tools.clone()),
        };
        let model = state.model.clone();
        let thinking_level = state.thinking_level.clone();
        drop(state);

        let emit = self.create_event_sink();
        let steering_queue = self.steering_queue.clone();
        let follow_up_queue = self.follow_up_queue.clone();

        let config = crate::agent_loop::AgentLoopConfig {
            model: model.clone(),
            reasoning: if thinking_level == "off".to_string() {
                None
            } else {
                Some(thinking_level)
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
            get_steering_messages: Some(Arc::new(move || {
                let q = steering_queue.clone();
                Box::pin(async move { q.lock().await.drain() })
            })),
            get_follow_up_messages: Some(Arc::new(move || {
                let q = follow_up_queue.clone();
                Box::pin(async move { q.lock().await.drain() })
            })),
            should_stop_after_turn: self.should_stop_after_turn.clone(),
            prepare_next_turn: self.prepare_next_turn.clone(),
            before_tool_call: self.before_tool_call.clone(),
            after_tool_call: self.after_tool_call.clone(),
            on_payload: self.on_payload.clone(),
            on_response: self.on_response.clone(),
        };

        let signal = Some(cancel_rx);

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

        match loop_result {
            Ok(messages) => Ok(messages),
            Err(e) => {
                let aborted = false;
                let failure_message = AgentMessage::Assistant {
                    content: vec![ContentBlock::Text { text: String::new(), text_signature: None }],
                    api: model.api.clone(),
                    provider: model.provider.clone(),
                    model: model.id.clone(),
                    usage: Usage::default(),
                    stop_reason: Some(if aborted { StopReason::Aborted } else { StopReason::Error }),
                    error_message: Some(e.to_string()),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                };
                emit(AgentEvent::MessageStart { message: failure_message.clone() }).await;
                emit(AgentEvent::MessageEnd { message: failure_message.clone() }).await;
                emit(AgentEvent::TurnEnd { message: failure_message.clone(), tool_results: Vec::new() }).await;
                emit(AgentEvent::AgentEnd { messages: vec![failure_message.clone()] }).await;
                Ok(vec![failure_message])
            }
        }
    }

    async fn run_with_lifecycle_continue(
        &self,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            cancel.cancelled().await;
            let _ = cancel_tx.send(true);
        });

        let active_run = ActiveRun {
            cancel: cancel_clone,
            handle,
        };
        *self.active_run.lock().await = Some(active_run);

        let state = self.state.read().await;
        let context = AgentContext {
            system_prompt: state.system_prompt.clone(),
            messages: state.messages.clone(),
            tools: Some(state.tools.clone()),
        };
        let model = state.model.clone();
        let thinking_level = state.thinking_level.clone();
        drop(state);

        let emit = self.create_event_sink();
        let steering_queue = self.steering_queue.clone();
        let follow_up_queue = self.follow_up_queue.clone();

        let config = crate::agent_loop::AgentLoopConfig {
            model,
            reasoning: if thinking_level == "off".to_string() {
                None
            } else {
                Some(thinking_level)
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
            get_steering_messages: Some(Arc::new(move || {
                let q = steering_queue.clone();
                Box::pin(async move { q.lock().await.drain() })
            })),
            get_follow_up_messages: Some(Arc::new(move || {
                let q = follow_up_queue.clone();
                Box::pin(async move { q.lock().await.drain() })
            })),
            should_stop_after_turn: self.should_stop_after_turn.clone(),
            prepare_next_turn: self.prepare_next_turn.clone(),
            before_tool_call: self.before_tool_call.clone(),
            after_tool_call: self.after_tool_call.clone(),
            on_payload: self.on_payload.clone(),
            on_response: self.on_response.clone(),
        };

        let signal = Some(cancel_rx);

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

    fn create_event_sink(&self) -> AgentEventSink {
        let listeners = self.listeners.clone();
        let state = self.state.clone();
        let idle_notify = self.idle_notify.clone();

        Arc::new(move |event: AgentEvent| {
            let listeners = listeners.clone();
            let state = state.clone();
            let idle_notify = idle_notify.clone();
            Box::pin(async move {
                {
                    let state_read = state.read().await;
                    match &event {
                        AgentEvent::MessageStart { message } => {
                            if matches!(message, AgentMessage::Assistant { .. }) {
                                drop(state_read);
                                let mut s = state.write().await;
                                s.streaming_message = Some(message.clone());
                            }
                        }
                        AgentEvent::MessageUpdate { message, .. } => {
                            drop(state_read);
                            let mut s = state.write().await;
                            s.streaming_message = Some(message.clone());
                        }
                        AgentEvent::MessageEnd { message } => {
                            drop(state_read);
                            let mut s = state.write().await;
                            s.streaming_message = None;
                            s.messages.push(message.clone());
                        }
                        AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
                            drop(state_read);
                            let mut s = state.write().await;
                            s.pending_tool_calls.insert(tool_call_id.clone());
                        }
                        AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
                            drop(state_read);
                            let mut s = state.write().await;
                            s.pending_tool_calls.remove(tool_call_id);
                        }
                        AgentEvent::TurnEnd { message, .. } => {
                            if let AgentMessage::Assistant { error_message: Some(err), .. } = message {
                                drop(state_read);
                                let mut s = state.write().await;
                                s.error_message = Some(err.clone());
                            }
                        }
                        AgentEvent::AgentEnd { .. } => {
                            drop(state_read);
                            let mut s = state.write().await;
                            s.is_streaming = false;
                            s.streaming_message = None;
                            drop(s);
                            idle_notify.notify_waiters();
                        }
                        _ => {}
                    }
                }

                let listeners = listeners.read().await;
                for listener in listeners.iter() {
                    listener(event.clone(), None).await;
                }
            })
        })
    }

    pub async fn set_model(&self, model: Model) {
        self.state.write().await.model = model;
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