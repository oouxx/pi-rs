
use crate::pi_ai_types::{ContentBlock, Message, StopReason, Usage};
use crate::types::{AgentContext, AgentEvent, AgentMessage, AgentToolCall, AgentToolResult};

#[derive(Debug, Clone)]
pub struct BeforeToolCallResult {
    pub block: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AfterToolCallResult {
    pub content: Option<Vec<ContentBlock>>,
    pub details: Option<serde_json::Value>,
    pub terminate: Option<bool>,
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PreparedToolCall {
    tool_call: AgentToolCall,
    args: serde_json::Value,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ImmediateToolCallOutcome {
    result: AgentToolResult<serde_json::Value>,
    is_error: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ExecutedToolCallOutcome {
    result: AgentToolResult<serde_json::Value>,
    is_error: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FinalizedToolCallOutcome {
    tool_call: AgentToolCall,
    result: AgentToolResult<serde_json::Value>,
    is_error: bool,
}

#[allow(dead_code)]
fn create_error_tool_result(message: &str) -> AgentToolResult<serde_json::Value> {
    AgentToolResult {
        content: vec![ContentBlock::Text {
            text: message.to_string(),
            text_signature: None,
        }],
        details: serde_json::Value::Null,
        terminate: None,
    }
}

#[allow(dead_code)]
fn should_terminate_tool_batch(finalized_calls: &[FinalizedToolCallOutcome]) -> bool {
    !finalized_calls.is_empty() && finalized_calls.iter().all(|f| f.result.terminate == Some(true))
}

#[allow(dead_code)]
fn validate_tool_arguments(
    tool_schema: &serde_json::Value,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    if tool_schema.is_null() || args.is_object() {
        Ok(args.clone())
    } else {
        Err(format!("Invalid arguments for tool: expected object, got {}", args))
    }
}

pub struct AgentLoopConfig {
    pub model: crate::pi_ai_types::Model,
    pub reasoning: Option<crate::pi_ai_types::ThinkingLevel>,
    pub session_id: String,
    pub tool_execution: crate::pi_ai_types::ToolExecutionMode,
    pub before_tool_call: Option<
        Box<
            dyn Fn(
                    AgentMessage,
                    AgentToolCall,
                    serde_json::Value,
                    AgentContext,
                ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<BeforeToolCallResult>> + Send>>
                + Send
                + Sync,
        >,
    >,
    pub after_tool_call: Option<
        Box<
            dyn Fn(
                    AgentMessage,
                    AgentToolCall,
                    serde_json::Value,
                    AgentToolResult<serde_json::Value>,
                    bool,
                    AgentContext,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Option<AgentToolResult<serde_json::Value>>> + Send>,
                > + Send
                + Sync,
        >,
    >,
    pub convert_to_llm: Option<Box<dyn Fn(&[AgentMessage]) -> Vec<Message> + Send + Sync>>,
    pub get_steering_messages:
        Box<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<AgentMessage>> + Send>> + Send + Sync>,
    pub get_follow_up_messages:
        Box<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<AgentMessage>> + Send>> + Send + Sync>,
}

pub type AgentEventSink = Box<
    dyn Fn(AgentEvent) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync,
>;

pub async fn run_agent_loop(
    context: AgentContext,
    _config: &AgentLoopConfig,
    emit: AgentEventSink,
    _signal: Option<tokio::sync::watch::Receiver<bool>>,
    stream_fn: Box<
        dyn Fn(
                crate::pi_ai_types::Model,
                crate::pi_ai_types::Context,
                Option<crate::pi_ai_types::ThinkingLevel>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<
                                crate::pi_ai_types::AssistantMessage,
                                Box<dyn std::error::Error + Send + Sync>,
                            >,
                        > + Send,
                >,
            > + Send
            + Sync,
    >,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    emit(AgentEvent::AgentStart).await;

    let pi_context = crate::pi_ai_types::Context {
        system_prompt: context.system_prompt.clone(),
        messages: crate::harness::messages::convert_to_llm(&context.messages),
        tools: None,
    };

    let result = stream_fn(_config.model.clone(), pi_context, _config.reasoning.clone()).await;

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

            emit(AgentEvent::TurnStart).await;
            emit(AgentEvent::MessageStart {
                message: agent_msg.clone(),
            })
            .await;
            emit(AgentEvent::MessageEnd {
                message: agent_msg.clone(),
            })
            .await;

            let tool_results = process_tool_calls_in_loop(&agent_msg, &context, _config, &emit).await;

            emit(AgentEvent::TurnEnd {
                message: agent_msg.clone(),
                tool_results,
            })
            .await;

            emit(AgentEvent::AgentEnd {
                messages: vec![agent_msg],
            })
            .await;
        }
        Err(e) => {
            let failure_message = AgentMessage::Assistant {
                content: vec![ContentBlock::Text {
                    text: String::new(),
                    text_signature: None,
                }],
                api: _config.model.api.clone(),
                provider: _config.model.provider.clone(),
                model: _config.model.id.clone(),
                usage: Usage::default(),
                stop_reason: Some(StopReason::Error),
                error_message: Some(e.to_string()),
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
    }

    Ok(())
}

async fn process_tool_calls_in_loop(
    assistant_msg: &AgentMessage,
    _context: &AgentContext,
    _config: &AgentLoopConfig,
    emit: &AgentEventSink,
) -> Vec<AgentMessage> {
    let content = match assistant_msg {
        AgentMessage::Assistant { content, .. } => content,
        _ => return Vec::new(),
    };

    let tool_calls: Vec<AgentToolCall> = content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
            } => Some(AgentToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            }),
            _ => None,
        })
        .collect();

    let mut results = Vec::new();
    for tc in &tool_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            args: tc.arguments.clone(),
        })
        .await;

        let tool_result_msg = AgentMessage::ToolResult {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            content: vec![ContentBlock::Text {
                text: format!("Tool {} executed", tc.name),
                text_signature: None,
            }],
            details: serde_json::Value::Null,
            is_error: false,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            result: serde_json::Value::Null,
            is_error: false,
        })
        .await;

        emit(AgentEvent::MessageStart {
            message: tool_result_msg.clone(),
        })
        .await;
        emit(AgentEvent::MessageEnd {
            message: tool_result_msg.clone(),
        })
        .await;

        results.push(tool_result_msg);
    }

    results
}

pub async fn run_agent_loop_continue(
    _context: AgentContext,
    _config: &AgentLoopConfig,
    _emit: AgentEventSink,
    _signal: Option<tokio::sync::watch::Receiver<bool>>,
    _stream_fn: Box<
        dyn Fn(
                crate::pi_ai_types::Model,
                crate::pi_ai_types::Context,
                Option<crate::pi_ai_types::ThinkingLevel>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<
                                crate::pi_ai_types::AssistantMessage,
                                Box<dyn std::error::Error + Send + Sync>,
                            >,
                        > + Send,
                >,
            > + Send
            + Sync,
    >,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(())
}