use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{watch, Mutex, RwLock};

use crate::pi_ai_types::{ContentBlock, Model, StopReason, ThinkingLevel, EMPTY_USAGE};
use crate::types::{AgentContext, AgentEvent, AgentMessage, AgentState};

type AgentEventListener = Box<
    dyn Fn(AgentEvent, Option<watch::Receiver<bool>>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

#[allow(dead_code)]
struct ActiveRun {
    tx: tokio::sync::oneshot::Sender<()>,
    abort_handle: tokio::task::JoinHandle<()>,
}

#[allow(dead_code)]
pub struct Agent {
    session_id: String,
    state: Arc<RwLock<AgentState>>,
    listeners: Arc<RwLock<Vec<AgentEventListener>>>,
    active_run: Arc<Mutex<Option<ActiveRun>>>,
    stream_fn: Arc<dyn Fn(Model, crate::pi_ai_types::Context, Option<crate::pi_ai_types::ThinkingLevel>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::pi_ai_types::AssistantMessage, Box<dyn std::error::Error + Send + Sync>>> + Send>> + Send + Sync>,
    convert_to_llm: Option<Arc<dyn Fn(&[AgentMessage]) -> Vec<crate::pi_ai_types::Message> + Send + Sync>>,
    before_tool_call: Option<Arc<dyn Fn(AgentMessage, crate::types::AgentToolCall, serde_json::Value, AgentContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<crate::agent_loop::BeforeToolCallResult>> + Send>> + Send + Sync>>,
    after_tool_call: Option<Arc<dyn Fn(AgentMessage, crate::types::AgentToolCall, serde_json::Value, crate::types::AgentToolResult<serde_json::Value>, bool, AgentContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<crate::types::AgentToolResult<serde_json::Value>>> + Send>> + Send + Sync>>,
    steering_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
}

impl Agent {
    pub fn new(
        session_id: String,
        model: Model,
        system_prompt: String,
        stream_fn: Arc<dyn Fn(Model, crate::pi_ai_types::Context, Option<crate::pi_ai_types::ThinkingLevel>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::pi_ai_types::AssistantMessage, Box<dyn std::error::Error + Send + Sync>>> + Send>> + Send + Sync>,
    ) -> Self {
        let state = AgentState {
            system_prompt,
            model,
            thinking_level: ThinkingLevel::Off,
            tools: Vec::new(),
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: HashSet::new(),
            error_message: None,
        };
        Self {
            session_id,
            state: Arc::new(RwLock::new(state)),
            listeners: Arc::new(RwLock::new(Vec::new())),
            active_run: Arc::new(Mutex::new(None)),
            stream_fn,
            convert_to_llm: None,
            before_tool_call: None,
            after_tool_call: None,
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn state(&self) -> AgentState {
        self.state.read().await.clone()
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

    pub async fn steer(&self, message: AgentMessage) {
        self.steering_queue.lock().await.push(message);
    }

    pub async fn follow_up(&self, message: AgentMessage) {
        self.follow_up_queue.lock().await.push(message);
    }

    pub async fn subscribe(
        &self,
        listener: AgentEventListener,
    ) {
        self.listeners.write().await.push(listener);
    }

    pub async fn abort(&self) {
        if let Some(run) = self.active_run.lock().await.take() {
            run.abort_handle.abort();
        }
    }

    pub async fn process(&self, messages: Vec<AgentMessage>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        {
            let mut state = self.state.write().await;
            state.messages.extend(messages);
        }
        self.run_agent_loop(false).await
    }

    pub async fn continue_run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.run_agent_loop(false).await
    }

    async fn run_agent_loop(
        &self,
        _skip_initial_steering: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state = self.state.read().await;
        let context = AgentContext {
            system_prompt: state.system_prompt.clone(),
            messages: state.messages.clone(),
            tools: Some(state.tools.clone()),
        };
        let model = state.model.clone();
        let thinking_level = state.thinking_level.clone();
        drop(state);

        let stream_fn = self.stream_fn.clone();
        let pi_context = crate::pi_ai_types::Context {
            system_prompt: context.system_prompt.clone(),
            messages: self.convert_messages(&context.messages),
            tools: None,
        };

        self.emit_event(AgentEvent::AgentStart).await;

        let result = stream_fn(model, pi_context, Some(thinking_level)).await;

        match result {
            Ok(assistant_msg) => {
                let agent_msg = AgentMessage::Assistant {
                    content: assistant_msg.content.clone(),
                    api: assistant_msg.api.clone(),
                    provider: assistant_msg.provider.clone(),
                    model: assistant_msg.model.clone(),
                    usage: assistant_msg.usage.clone(),
                    stop_reason: assistant_msg.stop_reason.clone(),
                    error_message: assistant_msg.error_message.clone(),
                    timestamp: assistant_msg.timestamp,
                };

                self.emit_event(AgentEvent::MessageStart {
                    message: agent_msg.clone(),
                })
                .await;
                self.emit_event(AgentEvent::MessageEnd {
                    message: agent_msg.clone(),
                })
                .await;

                {
                    let mut state = self.state.write().await;
                    state.messages.push(agent_msg.clone());
                }

                let tool_results = self.process_tool_calls(&agent_msg).await;

                self.emit_event(AgentEvent::TurnEnd {
                    message: agent_msg.clone(),
                    tool_results: tool_results.clone(),
                })
                .await;

                self.emit_event(AgentEvent::AgentEnd {
                    messages: vec![agent_msg],
                })
                .await;
            }
            Err(e) => {
                let state = self.state.read().await;
                let failure_message = AgentMessage::Assistant {
                    content: vec![ContentBlock::Text {
                        text: String::new(),
                        text_signature: None,
                    }],
                    api: state.model.api.clone(),
                    provider: state.model.provider.clone(),
                    model: state.model.id.clone(),
                    usage: EMPTY_USAGE,
                    stop_reason: Some(StopReason::Error),
                    error_message: Some(e.to_string()),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                };
                drop(state);

                self.emit_event(AgentEvent::MessageStart {
                    message: failure_message.clone(),
                })
                .await;
                self.emit_event(AgentEvent::MessageEnd {
                    message: failure_message.clone(),
                })
                .await;
                self.emit_event(AgentEvent::TurnEnd {
                    message: failure_message.clone(),
                    tool_results: Vec::new(),
                })
                .await;
                self.emit_event(AgentEvent::AgentEnd {
                    messages: vec![failure_message],
                })
                .await;
            }
        }

        {
            let mut state = self.state.write().await;
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
        }

        Ok(())
    }

    async fn process_tool_calls(&self, assistant_msg: &AgentMessage) -> Vec<AgentMessage> {
        let content = match assistant_msg {
            AgentMessage::Assistant { content, .. } => content,
            _ => return Vec::new(),
        };

        let tool_calls: Vec<_> = content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                } => Some(crate::types::AgentToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }),
                _ => None,
            })
            .collect();

        let mut results = Vec::new();
        for tc in tool_calls {
            self.emit_event(AgentEvent::ToolExecutionStart {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                args: tc.arguments.clone(),
            })
            .await;

            let result_content = vec![ContentBlock::Text {
                text: format!("Tool {} executed (stub)", tc.name),
                text_signature: None,
            }];

            let tool_result = AgentMessage::ToolResult {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                content: result_content,
                details: serde_json::Value::Null,
                is_error: false,
                timestamp: chrono::Utc::now().timestamp_millis(),
            };

            self.emit_event(AgentEvent::ToolExecutionEnd {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                result: serde_json::Value::Null,
                is_error: false,
            })
            .await;

            self.emit_event(AgentEvent::MessageStart {
                message: tool_result.clone(),
            })
            .await;
            self.emit_event(AgentEvent::MessageEnd {
                message: tool_result.clone(),
            })
            .await;

            {
                let mut state = self.state.write().await;
                state.messages.push(tool_result.clone());
            }

            results.push(tool_result);
        }

        results
    }

    fn convert_messages(&self, messages: &[AgentMessage]) -> Vec<crate::pi_ai_types::Message> {
        if let Some(convert_fn) = &self.convert_to_llm {
            return convert_fn(messages);
        }
        crate::harness::messages::convert_to_llm(messages)
    }

    async fn emit_event(&self, event: AgentEvent) {
        let listeners = self.listeners.read().await;
        for listener in listeners.iter() {
            listener(event.clone(), None).await;
        }
    }
}