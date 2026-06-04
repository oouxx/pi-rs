use std::sync::Arc;

use crate::pi_ai_types::{
    AssistantMessage, AssistantMessageEvent, ContentBlock, Model, StopReason,
    ThinkingLevel, ToolExecutionMode, Usage,
};
use crate::types::{
    AfterToolCallContext, AfterToolCallFn, AgentContext, AgentEvent,
    AgentEventSink, AgentMessage, AgentToolCall, AgentToolResult,
    BeforeToolCallContext, BeforeToolCallFn, ConvertToLlmFn,
    GetFollowUpMessagesFn, GetSteeringMessagesFn, PrepareNextTurnFn, ShouldStopAfterTurnContext,
    ShouldStopAfterTurnFn, StreamFn, StreamFnOptions, TransformContextFn,
};

enum PreparedOrImmediate {
    Prepared {
        tool_call: AgentToolCall,
        args: serde_json::Value,
    },
    Immediate {
        result: AgentToolResult<serde_json::Value>,
        is_error: bool,
    },
}

struct ExecutedToolCallOutcome {
    result: AgentToolResult<serde_json::Value>,
    is_error: bool,
}

struct FinalizedToolCallOutcome {
    tool_call: AgentToolCall,
    result: AgentToolResult<serde_json::Value>,
    is_error: bool,
}

struct ExecutedToolCallBatch {
    messages: Vec<AgentMessage>,
    terminate: bool,
}

fn create_error_tool_result(message: &str) -> AgentToolResult<serde_json::Value> {
    AgentToolResult {
        content: vec![ContentBlock::Text { text: message.to_string(), text_signature: None }],
        details: serde_json::Value::Object(Default::default()),
        terminate: None,
    }
}

fn should_terminate_tool_batch(finalized_calls: &[FinalizedToolCallOutcome]) -> bool {
    !finalized_calls.is_empty()
        && finalized_calls
            .iter()
            .all(|f| f.result.terminate == Some(true))
}

fn validate_tool_arguments(
    tool_schema: &serde_json::Value,
    args: &serde_json::Value,
) -> serde_json::Value {
    if tool_schema.is_null() {
        return args.clone();
    }
    if let Some(properties) = tool_schema.get("properties") {
        let mut filtered = serde_json::Map::new();
        if let Some(obj) = args.as_object() {
            for (key, value) in obj {
                if properties.get(key).is_some() {
                    filtered.insert(key.clone(), value.clone());
                }
            }
        }
        serde_json::Value::Object(filtered)
    } else if args.is_object() {
        args.clone()
    } else {
        args.clone()
    }
}

fn prepare_tool_call_arguments(
    tool: &crate::types::DynTool,
    tool_call: &AgentToolCall,
) -> AgentToolCall {
    if let Some(prepare) = &tool.prepare_arguments {
        let prepared = prepare(&tool_call.arguments);
        if serde_json::to_value(&prepared).unwrap_or_default() == tool_call.arguments {
            return tool_call.clone();
        }
        AgentToolCall {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: serde_json::to_value(&prepared).unwrap_or_else(|_| tool_call.arguments.clone()),
        }
    } else {
        tool_call.clone()
    }
}

async fn prepare_tool_call(
    current_context: &AgentContext,
    assistant_message: &AgentMessage,
    tool_call: &AgentToolCall,
    before_tool_call: &Option<BeforeToolCallFn>,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
) -> PreparedOrImmediate {
    let tool = current_context
        .tools
        .as_ref()
        .and_then(|tools| tools.iter().find(|t| t.name == tool_call.name));

    let tool = match tool {
        Some(t) => t,
        None => {
            return PreparedOrImmediate::Immediate {
                result: create_error_tool_result(&format!("Tool {} not found", tool_call.name)),
                is_error: true,
            };
        }
    };

    let prepared_tool_call = prepare_tool_call_arguments(tool, tool_call);
    let validated_args = validate_tool_arguments(&tool.parameters_schema, &prepared_tool_call.arguments);

    if let Some(before_fn) = before_tool_call {
        let ctx = BeforeToolCallContext {
            assistant_message: assistant_message.clone(),
            tool_call: tool_call.clone(),
            args: validated_args.clone(),
            context: current_context.clone(),
        };
        let result = before_fn(ctx, signal.clone()).await;
        if let Some(before_result) = result {
            if before_result.block {
                return PreparedOrImmediate::Immediate {
                    result: create_error_tool_result(
                        before_result
                            .reason
                            .as_deref()
                            .unwrap_or("Tool execution was blocked"),
                    ),
                    is_error: true,
                };
            }
        }
    }

    PreparedOrImmediate::Prepared {
        tool_call: prepared_tool_call,
        args: validated_args,
    }
}

async fn execute_prepared_tool_call(
    tool: &crate::types::DynTool,
    tool_call: &AgentToolCall,
    args: &serde_json::Value,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
    emit: &AgentEventSink,
) -> ExecutedToolCallOutcome {
    let tool_call_id = tool_call.id.clone();
    let tool_name = tool_call.name.clone();
    let args_clone = args.clone();
    let emit_clone = emit.clone();

    let on_update: Option<Arc<dyn Fn(AgentToolResult<serde_json::Value>) + Send + Sync>> =
        Some(Arc::new(move |partial_result| {
            let emit = emit_clone.clone();
            let tool_call_id = tool_call_id.clone();
            let tool_name = tool_name.clone();
            let args = args_clone.clone();
            tokio::spawn(async move {
                emit(AgentEvent::ToolExecutionUpdate {
                    tool_call_id,
                    tool_name,
                    args,
                    partial_result: serde_json::to_value(&partial_result).unwrap_or_default(),
                })
                .await;
            });
        }));

    match (tool.execute)(
        tool_call.id.clone(),
        args.clone(),
        signal.clone(),
        on_update,
    )
    .await
    {
        Ok(result) => ExecutedToolCallOutcome {
            result,
            is_error: false,
        },
        Err(e) => ExecutedToolCallOutcome {
            result: create_error_tool_result(&e.to_string()),
            is_error: true,
        },
    }
}

async fn finalize_executed_tool_call(
    current_context: &AgentContext,
    assistant_message: &AgentMessage,
    tool_call: &AgentToolCall,
    args: &serde_json::Value,
    executed: ExecutedToolCallOutcome,
    after_tool_call: &Option<AfterToolCallFn>,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
) -> FinalizedToolCallOutcome {
    let mut result = executed.result;
    let mut is_error = executed.is_error;

    if let Some(after_fn) = after_tool_call {
        let ctx = AfterToolCallContext {
            assistant_message: assistant_message.clone(),
            tool_call: tool_call.clone(),
            args: args.clone(),
            result: result.clone(),
            is_error,
            context: current_context.clone(),
        };
        match after_fn(ctx, signal.clone()).await {
            Some(after_result) => {
                if let Some(content) = after_result.content {
                    result.content = content;
                }
                if let Some(details) = after_result.details {
                    result.details = details;
                }
                if let Some(err) = after_result.is_error {
                    is_error = err;
                }
                if let Some(terminate) = after_result.terminate {
                    result.terminate = Some(terminate);
                }
            }
            None => {}
        }
    }

    FinalizedToolCallOutcome {
        tool_call: tool_call.clone(),
        result,
        is_error,
    }
}

fn create_tool_result_message(finalized: &FinalizedToolCallOutcome) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        content: finalized.result.content.clone(),
        details: finalized.result.details.clone(),
        is_error: finalized.is_error,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

async fn execute_tool_calls_sequential(
    current_context: &AgentContext,
    assistant_message: &AgentMessage,
    tool_calls: &[AgentToolCall],
    before_tool_call: &Option<BeforeToolCallFn>,
    after_tool_call: &Option<AfterToolCallFn>,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut finalized_calls: Vec<FinalizedToolCallOutcome> = Vec::new();
    let mut messages: Vec<AgentMessage> = Vec::new();

    for tool_call in tool_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: tool_call.arguments.clone(),
        })
        .await;

        let preparation = prepare_tool_call(
            current_context,
            assistant_message,
            tool_call,
            before_tool_call,
            signal,
        )
        .await;

        let finalized = match preparation {
            PreparedOrImmediate::Immediate { result, is_error } => FinalizedToolCallOutcome {
                tool_call: tool_call.clone(),
                result,
                is_error,
            },
            PreparedOrImmediate::Prepared { tool_call: tc, args } => {
                let tool = current_context
                    .tools
                    .as_ref()
                    .and_then(|tools| tools.iter().find(|t| t.name == tc.name))
                    .unwrap();

                let executed =
                    execute_prepared_tool_call(tool, &tc, &args, signal, emit).await;
                finalize_executed_tool_call(
                    current_context,
                    assistant_message,
                    &tc,
                    &args,
                    executed,
                    after_tool_call,
                    signal,
                )
                .await
            }
        };

        emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: finalized.tool_call.id.clone(),
            tool_name: finalized.tool_call.name.clone(),
            result: serde_json::to_value(&finalized.result).unwrap_or_default(),
            is_error: finalized.is_error,
        })
        .await;

        let tool_result_message = create_tool_result_message(&finalized);
        emit(AgentEvent::MessageStart {
            message: tool_result_message.clone(),
        })
        .await;
        emit(AgentEvent::MessageEnd {
            message: tool_result_message.clone(),
        })
        .await;

        finalized_calls.push(finalized);
        messages.push(tool_result_message);
    }

    ExecutedToolCallBatch {
        messages,
        terminate: should_terminate_tool_batch(&finalized_calls),
    }
}

async fn execute_tool_calls_parallel(
    current_context: &AgentContext,
    assistant_message: &AgentMessage,
    tool_calls: &[AgentToolCall],
    before_tool_call: &Option<BeforeToolCallFn>,
    after_tool_call: &Option<AfterToolCallFn>,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    enum FinalizedEntry {
        Done(FinalizedToolCallOutcome),
        Lazy(
            std::pin::Pin<Box<dyn std::future::Future<Output = FinalizedToolCallOutcome> + Send>>,
        ),
    }

    let mut entries: Vec<FinalizedEntry> = Vec::new();

    for tool_call in tool_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: tool_call.arguments.clone(),
        })
        .await;

        let preparation = prepare_tool_call(
            current_context,
            assistant_message,
            tool_call,
            before_tool_call,
            signal,
        )
        .await;

        match preparation {
            PreparedOrImmediate::Immediate { result, is_error } => {
                let finalized = FinalizedToolCallOutcome {
                    tool_call: tool_call.clone(),
                    result,
                    is_error,
                };
                emit(AgentEvent::ToolExecutionEnd {
                    tool_call_id: finalized.tool_call.id.clone(),
                    tool_name: finalized.tool_call.name.clone(),
                    result: serde_json::to_value(&finalized.result).unwrap_or_default(),
                    is_error: finalized.is_error,
                })
                .await;
                entries.push(FinalizedEntry::Done(finalized));
            }
            PreparedOrImmediate::Prepared { tool_call: tc, args } => {
                let tool = current_context
                    .tools
                    .as_ref()
                    .and_then(|tools| tools.iter().find(|t| t.name == tc.name))
                    .unwrap()
                    .clone();
                let ctx = current_context.clone();
                let msg = assistant_message.clone();
                let after_fn = after_tool_call.clone();
                let sig = signal.clone();
                let emit_c = emit.clone();

                let fut = Box::pin(async move {
                    let executed =
                        execute_prepared_tool_call(&tool, &tc, &args, &sig, &emit_c).await;
                    finalize_executed_tool_call(
                        &ctx,
                        &msg,
                        &tc,
                        &args,
                        executed,
                        &after_fn,
                        &sig,
                    )
                    .await
                });

                entries.push(FinalizedEntry::Lazy(fut));
            }
        }
    }

    let mut ordered_finalized: Vec<FinalizedToolCallOutcome> = Vec::new();
    for entry in entries {
        match entry {
            FinalizedEntry::Done(f) => {
                ordered_finalized.push(f);
            }
            FinalizedEntry::Lazy(fut) => {
                let finalized = fut.await;
                emit(AgentEvent::ToolExecutionEnd {
                    tool_call_id: finalized.tool_call.id.clone(),
                    tool_name: finalized.tool_call.name.clone(),
                    result: serde_json::to_value(&finalized.result).unwrap_or_default(),
                    is_error: finalized.is_error,
                })
                .await;
                ordered_finalized.push(finalized);
            }
        }
    }

    let mut messages: Vec<AgentMessage> = Vec::new();
    for finalized in &ordered_finalized {
        let tool_result_message = create_tool_result_message(finalized);
        emit(AgentEvent::MessageStart {
            message: tool_result_message.clone(),
        })
        .await;
        emit(AgentEvent::MessageEnd {
            message: tool_result_message.clone(),
        })
        .await;
        messages.push(tool_result_message);
    }

    ExecutedToolCallBatch {
        messages,
        terminate: should_terminate_tool_batch(&ordered_finalized),
    }
}

async fn execute_tool_calls(
    current_context: &AgentContext,
    assistant_message: &AgentMessage,
    tool_calls: &[AgentToolCall],
    tool_execution: ToolExecutionMode,
    before_tool_call: &Option<BeforeToolCallFn>,
    after_tool_call: &Option<AfterToolCallFn>,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    let has_sequential = tool_calls.iter().any(|tc| {
        current_context
            .tools
            .as_ref()
            .and_then(|tools| tools.iter().find(|t| t.name == tc.name))
            .and_then(|t| t.execution_mode)
            == Some(ToolExecutionMode::Sequential)
    });

    if tool_execution == ToolExecutionMode::Sequential || has_sequential {
        execute_tool_calls_sequential(
            current_context,
            assistant_message,
            tool_calls,
            before_tool_call,
            after_tool_call,
            signal,
            emit,
        )
        .await
    } else {
        execute_tool_calls_parallel(
            current_context,
            assistant_message,
            tool_calls,
            before_tool_call,
            after_tool_call,
            signal,
            emit,
        )
        .await
    }
}

fn extract_tool_calls(message: &AgentMessage) -> Vec<AgentToolCall> {
    match message {
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => Some(AgentToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

async fn stream_assistant_response(
    context: &mut AgentContext,
    model: &Model,
    thinking_level: Option<ThinkingLevel>,
    stream_fn: &StreamFn,
    stream_options: &StreamFnOptions,
    convert_to_llm: &ConvertToLlmFn,
    transform_context: &Option<TransformContextFn>,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
    emit: &AgentEventSink,
) -> Result<AssistantMessage, Box<dyn std::error::Error + Send + Sync>> {
    let mut messages = context.messages.clone();
    if let Some(transform) = transform_context {
        messages = transform(messages, signal.clone()).await;
    }

    let llm_messages = convert_to_llm(&messages);

    let pi_context = crate::pi_ai_types::Context {
        system_prompt: if context.system_prompt.is_empty() { None } else { Some(context.system_prompt.clone()) },
        messages: llm_messages,
        tools: context.tools.as_ref().map(|tools| {
            tools.iter().map(|t| crate::pi_ai_types::Tool {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters_schema.clone(),
            }).collect()
        }),
    };

    let stream = stream_fn(model.clone(), pi_context, thinking_level, stream_options.clone())
        .await?;

    let mut partial_message: Option<AssistantMessage> = None;
    let mut added_partial = false;

    let mut stream = Box::pin(stream);

    use futures::StreamExt;
    while let Some(event) = stream.next().await {
        match &event {
            AssistantMessageEvent::Start { partial } => {
                partial_message = Some(partial.clone());
                context.messages.push(agent_message_from_assistant(partial));
                added_partial = true;
                emit(AgentEvent::MessageStart {
                    message: agent_message_from_assistant(partial),
                })
                .await;
            }
            AssistantMessageEvent::TextStart { partial, .. }
            | AssistantMessageEvent::TextDelta { partial, .. }
            | AssistantMessageEvent::TextEnd { partial, .. }
            | AssistantMessageEvent::ThinkingStart { partial, .. }
            | AssistantMessageEvent::ThinkingDelta { partial, .. }
            | AssistantMessageEvent::ThinkingEnd { partial, .. }
            | AssistantMessageEvent::ToolCallStart { partial, .. }
            | AssistantMessageEvent::ToolCallDelta { partial, .. }
            | AssistantMessageEvent::ToolCallEnd { partial, .. } => {
                if partial_message.is_some() {
                    partial_message = Some(partial.clone());
                    let last_idx = context.messages.len() - 1;
                    context.messages[last_idx] = agent_message_from_assistant(partial);
                    emit(AgentEvent::MessageUpdate {
                        message: agent_message_from_assistant(partial),
                        assistant_message_event: event,
                    })
                    .await;
                }
            }
            AssistantMessageEvent::Done { message, .. }
            | AssistantMessageEvent::Error { error: message, .. } => {
                if added_partial {
                    let last_idx = context.messages.len() - 1;
                    context.messages[last_idx] = agent_message_from_assistant(message);
                } else {
                    context.messages.push(agent_message_from_assistant(message));
                }
                if !added_partial {
                    emit(AgentEvent::MessageStart {
                        message: agent_message_from_assistant(message),
                    })
                    .await;
                }
                emit(AgentEvent::MessageEnd {
                    message: agent_message_from_assistant(message),
                })
                .await;
                return Ok(message.clone());
            }
        }
    }

    let final_msg = partial_message.unwrap_or_else(|| AssistantMessage {
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        error_message: Some("Stream ended without done/error event".to_string()),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });

    if added_partial {
        let last_idx = context.messages.len() - 1;
        context.messages[last_idx] = agent_message_from_assistant(&final_msg);
    } else {
        context.messages.push(agent_message_from_assistant(&final_msg));
        emit(AgentEvent::MessageStart {
            message: agent_message_from_assistant(&final_msg),
        })
        .await;
    }
    emit(AgentEvent::MessageEnd {
        message: agent_message_from_assistant(&final_msg),
    })
    .await;

    Ok(final_msg)
}

fn agent_message_from_assistant(msg: &AssistantMessage) -> AgentMessage {
    AgentMessage::Assistant {
        content: msg.content.clone(),
        api: msg.api.clone(),
        provider: msg.provider.clone(),
        model: msg.model.clone(),
        usage: msg.usage.clone(),
        stop_reason: Some(msg.stop_reason.clone()),
        error_message: msg.error_message.clone(),
        timestamp: msg.timestamp,
    }
}

pub struct AgentLoopConfig {
    pub model: Model,
    pub reasoning: Option<ThinkingLevel>,
    pub api_key: Option<String>,
    pub session_id: Option<String>,
    pub thinking_budgets: Option<crate::pi_ai_types::ThinkingBudgets>,
    pub transport: Option<String>,
    pub max_retry_delay_ms: Option<u64>,
    pub tool_execution: ToolExecutionMode,
    pub convert_to_llm: ConvertToLlmFn,
    pub transform_context: Option<TransformContextFn>,
    pub get_api_key: Option<crate::types::GetApiKeyFn>,
    pub get_steering_messages: Option<GetSteeringMessagesFn>,
    pub get_follow_up_messages: Option<GetFollowUpMessagesFn>,
    pub should_stop_after_turn: Option<ShouldStopAfterTurnFn>,
    pub prepare_next_turn: Option<PrepareNextTurnFn>,
    pub before_tool_call: Option<BeforeToolCallFn>,
    pub after_tool_call: Option<AfterToolCallFn>,
    pub on_payload: Option<Arc<dyn Fn(serde_json::Value) + Send + Sync>>,
    pub on_response: Option<Arc<dyn Fn(&AssistantMessage) + Send + Sync>>,
    pub max_consecutive_tool_calls: Option<usize>,
}

pub async fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: &AgentLoopConfig,
    emit: &AgentEventSink,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
    stream_fn: &StreamFn,
) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
    let mut new_messages: Vec<AgentMessage> = prompts.clone();
    let mut current_context = AgentContext {
        system_prompt: context.system_prompt.clone(),
        messages: [&context.messages[..], &prompts[..]].concat(),
        tools: context.tools.clone(),
    };

    emit(AgentEvent::AgentStart).await;
    emit(AgentEvent::TurnStart).await;

    for prompt in &prompts {
        emit(AgentEvent::MessageStart {
            message: prompt.clone(),
        })
        .await;
        emit(AgentEvent::MessageEnd {
            message: prompt.clone(),
        })
        .await;
    }

    run_loop(
        &mut current_context,
        &mut new_messages,
        config,
        signal,
        emit,
        stream_fn,
    )
    .await?;

    Ok(new_messages)
}

pub async fn run_agent_loop_continue(
    context: AgentContext,
    config: &AgentLoopConfig,
    emit: &AgentEventSink,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
    stream_fn: &StreamFn,
) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
    if context.messages.is_empty() {
        return Err("Cannot continue: no messages in context".into());
    }
    if context.messages.last().map(|m| m.role()) == Some("assistant") {
        return Err("Cannot continue from message role: assistant".into());
    }

    let mut current_context = context;
    let mut new_messages: Vec<AgentMessage> = Vec::new();

    emit(AgentEvent::AgentStart).await;
    emit(AgentEvent::TurnStart).await;

    run_loop(
        &mut current_context,
        &mut new_messages,
        config,
        signal,
        emit,
        stream_fn,
    )
    .await?;

    Ok(new_messages)
}

async fn run_loop(
    current_context: &mut AgentContext,
    new_messages: &mut Vec<AgentMessage>,
    initial_config: &AgentLoopConfig,
    signal: &Option<tokio::sync::watch::Receiver<bool>>,
    emit: &AgentEventSink,
    stream_fn: &StreamFn,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config_model = initial_config.model.clone();
    let mut config_reasoning = initial_config.reasoning.clone();
    let mut first_turn = true;

    let max_consecutive_tool_calls = initial_config.max_consecutive_tool_calls.unwrap_or(25);
    let mut consecutive_tool_call_rounds: usize = 0;

    let mut pending_messages: Vec<AgentMessage> = if let Some(get_steering) = &initial_config.get_steering_messages {
        get_steering().await
    } else {
        Vec::new()
    };

    loop {
        let mut has_more_tool_calls = true;

        while has_more_tool_calls || !pending_messages.is_empty() {
            if !first_turn {
                emit(AgentEvent::TurnStart).await;
            } else {
                first_turn = false;
            }

            if !pending_messages.is_empty() {
                for message in pending_messages.drain(..) {
                    emit(AgentEvent::MessageStart {
                        message: message.clone(),
                    })
                    .await;
                    emit(AgentEvent::MessageEnd {
                        message: message.clone(),
                    })
                    .await;
                    current_context.messages.push(message.clone());
                    new_messages.push(message);
                }
            }

            let resolved_api_key = if let Some(get_api_key) = &initial_config.get_api_key {
                get_api_key(config_model.provider.clone()).await
            } else {
                None
            }
            .or_else(|| initial_config.api_key.clone());

            let stream_options = StreamFnOptions {
                api_key: resolved_api_key,
                session_id: initial_config.session_id.clone(),
                thinking_budgets: initial_config.thinking_budgets.clone(),
                transport: initial_config.transport.clone(),
                max_retry_delay_ms: initial_config.max_retry_delay_ms,
                signal: signal.clone(),
                on_payload: initial_config.on_payload.clone(),
                on_response: initial_config.on_response.clone(),
                ..Default::default()
            };

            let message = stream_assistant_response(
                current_context,
                &config_model,
                config_reasoning.clone(),
                stream_fn,
                &stream_options,
                &initial_config.convert_to_llm,
                &initial_config.transform_context,
                signal,
                emit,
            )
            .await?;

            let agent_msg = agent_message_from_assistant(&message);
            new_messages.push(agent_msg.clone());

            let stop_reason = message.stop_reason.clone();
            match stop_reason {
                StopReason::Error | StopReason::Aborted => {
                    emit(AgentEvent::TurnEnd {
                        message: agent_msg,
                        tool_results: Vec::new(),
                    })
                    .await;
                    emit(AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    })
                    .await;
                    return Ok(());
                }
                _ => {}
            }

            let tool_calls = extract_tool_calls(&agent_msg);
            has_more_tool_calls = !tool_calls.is_empty();

            if has_more_tool_calls {
                consecutive_tool_call_rounds += 1;
                if consecutive_tool_call_rounds > max_consecutive_tool_calls {
                    let err_msg = format!(
                        "Agent terminated: exceeded {} consecutive tool call rounds without producing a text response",
                        max_consecutive_tool_calls
                    );
                    eprintln!("[agent_loop] {}", err_msg);
                    break;
                }
            } else {
                consecutive_tool_call_rounds = 0;
            }

            let tool_results = if !tool_calls.is_empty() {
                let batch = execute_tool_calls(
                    current_context,
                    &agent_msg,
                    &tool_calls,
                    initial_config.tool_execution,
                    &initial_config.before_tool_call,
                    &initial_config.after_tool_call,
                    signal,
                    emit,
                )
                .await;

                for msg in &batch.messages {
                    current_context.messages.push(msg.clone());
                    new_messages.push(msg.clone());
                }

                if batch.terminate {
                    emit(AgentEvent::TurnEnd {
                        message: agent_msg.clone(),
                        tool_results: batch.messages.clone(),
                    })
                    .await;
                    emit(AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    })
                    .await;
                    return Ok(());
                }

                batch.messages
            } else {
                Vec::new()
            };

            emit(AgentEvent::TurnEnd {
                message: agent_msg.clone(),
                tool_results: tool_results.clone(),
            })
            .await;

            let next_turn_context = ShouldStopAfterTurnContext {
                message: agent_msg.clone(),
                tool_results: tool_results.clone(),
                context: current_context.clone(),
                new_messages: new_messages.clone(),
            };

            if let Some(prepare_next_turn) = &initial_config.prepare_next_turn {
                if let Some(update) = prepare_next_turn(next_turn_context.clone()).await {
                    if let Some(ctx) = update.context {
                        *current_context = ctx;
                    }
                    if let Some(m) = update.model {
                        config_model = m;
                    }
                    if let Some(tl) = update.thinking_level {
                        config_reasoning = Some(tl);
                    }
                }
            }

            if let Some(should_stop) = &initial_config.should_stop_after_turn {
                if should_stop(next_turn_context).await {
                    emit(AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    })
                    .await;
                    return Ok(());
                }
            }

            pending_messages = if let Some(get_steering) = &initial_config.get_steering_messages {
                get_steering().await
            } else {
                Vec::new()
            };
        }

        let follow_up_messages = if let Some(get_follow_up) = &initial_config.get_follow_up_messages {
            get_follow_up().await
        } else {
            Vec::new()
        };

        if !follow_up_messages.is_empty() {
            pending_messages = follow_up_messages;
            continue;
        }

        break;
    }

    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_ai_types::{ContentBlock, StopReason, ToolExecutionMode, Usage};
    use crate::types::{
        AfterToolCallFn, AfterToolCallResult, AgentTool, BeforeToolCallFn,
        BeforeToolCallResult, DynTool,
    };
    use std::sync::Arc;

    fn dummy_tool_call(id: &str, name: &str, args: serde_json::Value) -> AgentToolCall {
        AgentToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    fn dummy_assistant_message() -> AgentMessage {
        AgentMessage::Assistant {
            content: vec![],
            api: "test-api".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
            usage: Usage::default(),
            stop_reason: Some(StopReason::Stop),
            error_message: None,
            timestamp: 1000,
        }
    }

    fn dummy_context(tools: Vec<Arc<DynTool>>) -> AgentContext {
        AgentContext {
            system_prompt: String::new(),
            messages: vec![],
            tools: Some(tools),
        }
    }

    fn dummy_agent_tool(
        name: &str,
        schema: serde_json::Value,
        prepare: Option<Arc<dyn Fn(&serde_json::Value) -> serde_json::Value + Send + Sync>>,
        execute_result: Result<AgentToolResult<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>>,
    ) -> Arc<DynTool> {
        let results: Vec<Result<AgentToolResult<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>>> =
            vec![execute_result];
        let exec_results = std::sync::Mutex::new(results);
        Arc::new(AgentTool {
            name: name.to_string(),
            description: String::new(),
            label: name.to_string(),
            parameters_schema: schema,
            execution_mode: None,
            prepare_arguments: prepare,
            execute: Arc::new(move |_id, _args, _signal, _on_update| {
                let mut guard = exec_results.lock().unwrap();
                let result = guard.remove(0);
                Box::pin(async move { result })
            }),
        })
    }

    fn dummy_event_sink() -> AgentEventSink {
        Arc::new(|_event: AgentEvent| Box::pin(async {}))
    }

    // ============================================================
    // create_error_tool_result
    // ============================================================
    #[test]
    fn test_create_error_tool_result_sync() {
        let result = create_error_tool_result("something went wrong");
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "something went wrong"),
            _ => panic!("expected Text block"),
        }
        assert_eq!(result.details, serde_json::json!({}));
        assert!(result.terminate.is_none());
    }

    // ============================================================
    // should_terminate_tool_batch
    // ============================================================
    #[test]
    fn test_should_terminate_empty() {
        assert!(!should_terminate_tool_batch(&[]));
    }

    #[test]
    fn test_should_terminate_all_true() {
        let calls = vec![
            FinalizedToolCallOutcome {
                tool_call: dummy_tool_call("1", "t1", serde_json::json!({})),
                result: AgentToolResult {
                    content: vec![],
                    details: serde_json::json!({}),
                    terminate: Some(true),
                },
                is_error: false,
            },
            FinalizedToolCallOutcome {
                tool_call: dummy_tool_call("2", "t2", serde_json::json!({})),
                result: AgentToolResult {
                    content: vec![],
                    details: serde_json::json!({}),
                    terminate: Some(true),
                },
                is_error: false,
            },
        ];
        assert!(should_terminate_tool_batch(&calls));
    }

    #[test]
    fn test_should_terminate_partial() {
        let calls = vec![
            FinalizedToolCallOutcome {
                tool_call: dummy_tool_call("1", "t1", serde_json::json!({})),
                result: AgentToolResult {
                    content: vec![],
                    details: serde_json::json!({}),
                    terminate: Some(true),
                },
                is_error: false,
            },
            FinalizedToolCallOutcome {
                tool_call: dummy_tool_call("2", "t2", serde_json::json!({})),
                result: AgentToolResult {
                    content: vec![],
                    details: serde_json::json!({}),
                    terminate: None,
                },
                is_error: false,
            },
        ];
        assert!(!should_terminate_tool_batch(&calls));
    }

    #[test]
    fn test_should_terminate_none() {
        let calls = vec![
            FinalizedToolCallOutcome {
                tool_call: dummy_tool_call("1", "t1", serde_json::json!({})),
                result: AgentToolResult {
                    content: vec![],
                    details: serde_json::json!({}),
                    terminate: None,
                },
                is_error: false,
            },
        ];
        assert!(!should_terminate_tool_batch(&calls));
    }

    // ============================================================
    // validate_tool_arguments
    // ============================================================
    #[test]
    fn test_validate_null_schema() {
        let args = serde_json::json!({"key": "value", "extra": 42});
        assert_eq!(
            validate_tool_arguments(&serde_json::Value::Null, &args),
            args
        );
    }

    #[test]
    fn test_validate_filters_extra_keys() {
        let schema = serde_json::json!({
            "properties": {
                "name": {"type": "string"}
            }
        });
        let args = serde_json::json!({"name": "foo", "extra": "bar"});
        let filtered = validate_tool_arguments(&schema, &args);
        assert_eq!(filtered, serde_json::json!({"name": "foo"}));
        assert!(filtered.get("extra").is_none());
    }

    #[test]
    fn test_validate_preserves_valid_keys() {
        let schema = serde_json::json!({
            "properties": {
                "a": {},
                "b": {}
            }
        });
        let args = serde_json::json!({"a": 1, "b": 2});
        assert_eq!(
            validate_tool_arguments(&schema, &args),
            serde_json::json!({"a": 1, "b": 2})
        );
    }

    #[test]
    fn test_validate_non_object_args() {
        let schema = serde_json::json!({"properties": {"x": {}}});
        let args = serde_json::json!("string_arg");
        // When schema has properties but args is not an object, returns empty object
        assert_eq!(validate_tool_arguments(&schema, &args), serde_json::json!({}));
    }

    #[test]
    fn test_validate_schema_no_properties() {
        let schema = serde_json::json!({"type": "object"});
        let args = serde_json::json!({"keep": "me"});
        assert_eq!(validate_tool_arguments(&schema, &args), args);
    }

    // ============================================================
    // prepare_tool_call_arguments
    // ============================================================
    #[test]
    fn test_prepare_arguments_no_prepare_fn() {
        let tool = dummy_agent_tool("test", serde_json::json!({}), None, Ok(AgentToolResult {
            content: vec![],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let call = dummy_tool_call("1", "test", serde_json::json!({"a": 1}));
        let result = prepare_tool_call_arguments(&tool, &call);
        assert_eq!(result.arguments, serde_json::json!({"a": 1}));
    }

    #[test]
    fn test_prepare_arguments_with_transform() {
        let prepare: Arc<dyn Fn(&serde_json::Value) -> serde_json::Value + Send + Sync> = Arc::new(|args| {
            let mut map = serde_json::Map::new();
            map.insert("transformed".to_string(), args["input"].clone());
            serde_json::Value::Object(map)
        });
        let tool = dummy_agent_tool("test", serde_json::json!({}), Some(prepare), Ok(AgentToolResult {
            content: vec![],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let call = dummy_tool_call("1", "test", serde_json::json!({"input": "hello"}));
        let result = prepare_tool_call_arguments(&tool, &call);
        assert_eq!(result.arguments, serde_json::json!({"transformed": "hello"}));
    }

    // ============================================================
    // extract_tool_calls
    // ============================================================
    #[test]
    fn test_extract_tool_calls_from_assistant() {
        let msg = AgentMessage::Assistant {
            content: vec![
                ContentBlock::Text { text: "thinking...".to_string(), text_signature: None },
                ContentBlock::ToolCall {
                    id: "tc-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "foo.txt"}),
                    thought_signature: None,
                },
                ContentBlock::ToolCall {
                    id: "tc-2".to_string(),
                    name: "write_file".to_string(),
                    arguments: serde_json::json!({"path": "bar.txt", "content": "hello"}),
                    thought_signature: None,
                },
            ],
            api: "test".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
            usage: Usage::default(),
            stop_reason: None,
            error_message: None,
            timestamp: 1000,
        };
        let calls = extract_tool_calls(&msg);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "tc-1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[1].id, "tc-2");
        assert_eq!(calls[1].name, "write_file");
    }

    #[test]
    fn test_extract_tool_calls_no_calls() {
        let msg = AgentMessage::Assistant {
            content: vec![ContentBlock::Text { text: "just text".to_string(), text_signature: None }],
            api: "test".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
            usage: Usage::default(),
            stop_reason: None,
            error_message: None,
            timestamp: 1000,
        };
        assert!(extract_tool_calls(&msg).is_empty());
    }

    #[test]
    fn test_extract_tool_calls_non_assistant() {
        let msg = AgentMessage::User {
            content: vec![],
            timestamp: 1000,
        };
        assert!(extract_tool_calls(&msg).is_empty());
    }

    // ============================================================
    // create_tool_result_message
    // ============================================================
    #[test]
    fn test_create_tool_result_message_content() {
        let finalized = FinalizedToolCallOutcome {
            tool_call: dummy_tool_call("call-1", "my_tool", serde_json::json!({"x": 1})),
            result: AgentToolResult {
                content: vec![ContentBlock::Text { text: "done".to_string(), text_signature: None }],
                details: serde_json::json!({"ok": true}),
                terminate: None,
            },
            is_error: false,
        };
        let msg = create_tool_result_message(&finalized);
        match msg {
            AgentMessage::ToolResult { tool_call_id, tool_name, content, details, is_error, .. } => {
                assert_eq!(tool_call_id, "call-1");
                assert_eq!(tool_name, "my_tool");
                assert!(!is_error);
                assert_eq!(details, serde_json::json!({"ok": true}));
                assert_eq!(content.len(), 1);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_create_tool_result_message_error() {
        let finalized = FinalizedToolCallOutcome {
            tool_call: dummy_tool_call("call-2", "err_tool", serde_json::json!({})),
            result: AgentToolResult {
                content: vec![],
                details: serde_json::json!({}),
                terminate: None,
            },
            is_error: true,
        };
        let msg = create_tool_result_message(&finalized);
        match msg {
            AgentMessage::ToolResult { is_error, .. } => assert!(is_error),
            _ => panic!("expected ToolResult"),
        }
    }

    // ============================================================
    // prepare_tool_call  (async)
    // ============================================================
    #[tokio::test]
    async fn test_prepare_tool_call_tool_not_found() {
        let ctx = dummy_context(vec![]);
        let call = dummy_tool_call("1", "nonexistent", serde_json::json!({}));

        let result = prepare_tool_call(
            &ctx,
            &dummy_assistant_message(),
            &call,
            &None,
            &None,
        )
        .await;

        match result {
            PreparedOrImmediate::Immediate { is_error, .. } => assert!(is_error),
            _ => panic!("expected Immediate for missing tool"),
        }
    }

    #[tokio::test]
    async fn test_prepare_tool_call_before_hook_blocks() {
        let tool = dummy_agent_tool("my_tool", serde_json::json!({}), None, Ok(AgentToolResult {
            content: vec![],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let ctx = dummy_context(vec![tool]);

        let before_fn: BeforeToolCallFn = Arc::new(|_ctx, _signal| {
            Box::pin(async {
                Some(BeforeToolCallResult {
                    block: true,
                    reason: Some("not allowed".to_string()),
                })
            })
        });

        let call = dummy_tool_call("1", "my_tool", serde_json::json!({}));
        let result = prepare_tool_call(
            &ctx,
            &dummy_assistant_message(),
            &call,
            &Some(before_fn),
            &None,
        )
        .await;

        match result {
            PreparedOrImmediate::Immediate { is_error, .. } => {
                assert!(is_error);
            }
            _ => panic!("expected Immediate when blocked"),
        }
    }

    #[tokio::test]
    async fn test_prepare_tool_call_before_hook_allows() {
        let schema = serde_json::json!({"properties": {"x": {}}});
        let tool = dummy_agent_tool("my_tool", schema, None, Ok(AgentToolResult {
            content: vec![],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let ctx = dummy_context(vec![tool]);

        let before_fn: BeforeToolCallFn = Arc::new(|_ctx, _signal| {
            Box::pin(async { None })
        });

        let call = dummy_tool_call("1", "my_tool", serde_json::json!({"x": 1}));
        let result = prepare_tool_call(
            &ctx,
            &dummy_assistant_message(),
            &call,
            &Some(before_fn),
            &None,
        )
        .await;

        match result {
            PreparedOrImmediate::Prepared { args, .. } => {
                assert_eq!(args, serde_json::json!({"x": 1}));
            }
            _ => panic!("expected Prepared"),
        }
    }

    // ============================================================
    // execute_prepared_tool_call
    // ============================================================
    #[tokio::test]
    async fn test_execute_prepared_success() {
        let execute_result = Ok(AgentToolResult {
            content: vec![ContentBlock::Text { text: "output".to_string(), text_signature: None }],
            details: serde_json::json!({"status": "ok"}),
            terminate: Some(false),
        });
        let tool = dummy_agent_tool("t1", serde_json::json!({}), None, execute_result);
        let call = dummy_tool_call("1", "t1", serde_json::json!({}));

        let outcome = execute_prepared_tool_call(
            &tool, &call, &serde_json::json!({}), &None, &dummy_event_sink(),
        )
        .await;

        assert!(!outcome.is_error);
        assert_eq!(outcome.result.content.len(), 1);
        assert_eq!(outcome.result.terminate, Some(false));
    }

    #[tokio::test]
    async fn test_execute_prepared_error() {
        let execute_result: Result<AgentToolResult<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> =
            Err("execution failed".into());
        let tool = dummy_agent_tool("t1", serde_json::json!({}), None, execute_result);
        let call = dummy_tool_call("1", "t1", serde_json::json!({}));

        let outcome = execute_prepared_tool_call(
            &tool, &call, &serde_json::json!({}), &None, &dummy_event_sink(),
        )
        .await;

        assert!(outcome.is_error);
        assert_eq!(outcome.result.content.len(), 1);
        match &outcome.result.content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "execution failed"),
            _ => panic!("expected error text"),
        }
    }

    // ============================================================
    // finalize_executed_tool_call
    // ============================================================
    #[tokio::test]
    async fn test_finalize_no_hook() {
        let executed = ExecutedToolCallOutcome {
            result: AgentToolResult {
                content: vec![],
                details: serde_json::json!({}),
                terminate: None,
            },
            is_error: false,
        };
        let ctx = dummy_context(vec![]);
        let call = dummy_tool_call("1", "t1", serde_json::json!({}));

        let finalized = finalize_executed_tool_call(
            &ctx, &dummy_assistant_message(), &call, &serde_json::json!({}),
            executed, &None, &None,
        )
        .await;

        assert!(!finalized.is_error);
        assert!(finalized.result.terminate.is_none());
    }

    #[tokio::test]
    async fn test_finalize_after_hook_modifies_result() {
        let executed = ExecutedToolCallOutcome {
            result: AgentToolResult {
                content: vec![],
                details: serde_json::json!({"original": true}),
                terminate: None,
            },
            is_error: false,
        };

        let after_fn: AfterToolCallFn = Arc::new(|_ctx, _signal| {
            Box::pin(async {
                Some(AfterToolCallResult {
                    content: Some(vec![ContentBlock::Text { text: "modified".to_string(), text_signature: None }]),
                    details: Some(serde_json::json!({"modified": true})),
                    is_error: Some(true),
                    terminate: Some(true),
                })
            })
        });

        let ctx = dummy_context(vec![]);
        let call = dummy_tool_call("1", "t1", serde_json::json!({}));

        let finalized = finalize_executed_tool_call(
            &ctx, &dummy_assistant_message(), &call, &serde_json::json!({}),
            executed, &Some(after_fn), &None,
        )
        .await;

        assert!(finalized.is_error);
        assert_eq!(finalized.result.terminate, Some(true));
        assert_eq!(finalized.result.details, serde_json::json!({"modified": true}));
        match &finalized.result.content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "modified"),
            _ => panic!("expected modified text"),
        }
    }

    // ============================================================
    // execute_tool_calls_sequential
    // ============================================================
    #[tokio::test]
    async fn test_sequential_basic() {
        let tool = dummy_agent_tool("adder", serde_json::json!({"properties": {"x": {}, "y": {}}}), None, Ok(AgentToolResult {
            content: vec![ContentBlock::Text { text: "3".to_string(), text_signature: None }],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let ctx = dummy_context(vec![tool]);
        let calls = vec![
            dummy_tool_call("1", "adder", serde_json::json!({"x": 1, "y": 2})),
        ];
        let msg = dummy_assistant_message();

        let batch = execute_tool_calls_sequential(
            &ctx, &msg, &calls, &None, &None, &None, &dummy_event_sink(),
        )
        .await;

        assert_eq!(batch.messages.len(), 1);
        assert!(!batch.terminate);
        match &batch.messages[0] {
            AgentMessage::ToolResult { tool_name, is_error, .. } => {
                assert_eq!(tool_name, "adder");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn test_sequential_terminate_all() {
        let tool = dummy_agent_tool("exit", serde_json::json!({}), None, Ok(AgentToolResult {
            content: vec![],
            details: serde_json::json!({}),
            terminate: Some(true),
        }));
        let ctx = dummy_context(vec![tool]);
        let calls = vec![dummy_tool_call("1", "exit", serde_json::json!({}))];
        let msg = dummy_assistant_message();

        let batch = execute_tool_calls_sequential(
            &ctx, &msg, &calls, &None, &None, &None, &dummy_event_sink(),
        )
        .await;

        assert!(batch.terminate);
    }

    #[tokio::test]
    async fn test_sequential_error_continues() {
        let err_result: Result<AgentToolResult<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> =
            Err("fail".into());
        let ok_result = Ok(AgentToolResult {
            content: vec![ContentBlock::Text { text: "ok".to_string(), text_signature: None }],
            details: serde_json::json!({}),
            terminate: None,
        });

        let err_tool = dummy_agent_tool("err", serde_json::json!({}), None, err_result);
        let ok_tool = dummy_agent_tool("ok", serde_json::json!({}), None, ok_result);
        let ctx = dummy_context(vec![err_tool, ok_tool]);
        let calls = vec![
            dummy_tool_call("1", "err", serde_json::json!({})),
            dummy_tool_call("2", "ok", serde_json::json!({})),
        ];
        let msg = dummy_assistant_message();

        let batch = execute_tool_calls_sequential(
            &ctx, &msg, &calls, &None, &None, &None, &dummy_event_sink(),
        )
        .await;

        assert_eq!(batch.messages.len(), 2);
        match &batch.messages[0] {
            AgentMessage::ToolResult { is_error, .. } => assert!(is_error),
            _ => panic!("expected ToolResult"),
        }
        match &batch.messages[1] {
            AgentMessage::ToolResult { is_error, .. } => assert!(!is_error),
            _ => panic!("expected ToolResult"),
        }
    }

    // ============================================================
    // execute_tool_calls_parallel
    // ============================================================
    #[tokio::test]
    async fn test_parallel_basic() {
        let tool_a = dummy_agent_tool("greet_a", serde_json::json!({}), None, Ok(AgentToolResult {
            content: vec![ContentBlock::Text { text: "hi".to_string(), text_signature: None }],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let tool_b = dummy_agent_tool("greet_b", serde_json::json!({}), None, Ok(AgentToolResult {
            content: vec![ContentBlock::Text { text: "hi".to_string(), text_signature: None }],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let ctx = dummy_context(vec![tool_a, tool_b]);
        let calls = vec![
            dummy_tool_call("1", "greet_a", serde_json::json!({})),
            dummy_tool_call("2", "greet_b", serde_json::json!({})),
        ];
        let msg = dummy_assistant_message();

        let batch = execute_tool_calls_parallel(
            &ctx, &msg, &calls, &None, &None, &None, &dummy_event_sink(),
        )
        .await;

        assert_eq!(batch.messages.len(), 2);
        assert!(!batch.terminate);
    }

    #[tokio::test]
    async fn test_parallel_mixed_immediate_and_lazy() {
        // First call: tool not found -> Immediate error
        // Second call: valid tool -> Lazy execution
        let found_tool = dummy_agent_tool("found", serde_json::json!({}), None, Ok(AgentToolResult {
            content: vec![ContentBlock::Text { text: "found".to_string(), text_signature: None }],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let ctx = dummy_context(vec![found_tool]);
        let calls = vec![
            dummy_tool_call("1", "missing", serde_json::json!({})),
            dummy_tool_call("2", "found", serde_json::json!({})),
        ];
        let msg = dummy_assistant_message();

        let batch = execute_tool_calls_parallel(
            &ctx, &msg, &calls, &None, &None, &None, &dummy_event_sink(),
        )
        .await;

        assert_eq!(batch.messages.len(), 2);
        match &batch.messages[0] {
            AgentMessage::ToolResult { tool_name, is_error, .. } => {
                assert_eq!(tool_name, "missing");
                assert!(is_error);
            }
            _ => panic!("expected ToolResult"),
        }
        match &batch.messages[1] {
            AgentMessage::ToolResult { tool_name, is_error, .. } => {
                assert_eq!(tool_name, "found");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    // ============================================================
    // execute_tool_calls  (dispatcher)
    // ============================================================
    #[tokio::test]
    async fn test_dispatcher_parallel_by_default() {
        let tool = dummy_agent_tool("t1", serde_json::json!({}), None, Ok(AgentToolResult {
            content: vec![],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let ctx = dummy_context(vec![tool]);
        let calls = vec![dummy_tool_call("1", "t1", serde_json::json!({}))];
        let msg = dummy_assistant_message();

        let batch = execute_tool_calls(
            &ctx, &msg, &calls,
            ToolExecutionMode::Parallel, &None, &None, &None, &dummy_event_sink(),
        )
        .await;

        assert_eq!(batch.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_dispatcher_sequential_mode() {
        let tool = dummy_agent_tool("t1", serde_json::json!({}), None, Ok(AgentToolResult {
            content: vec![],
            details: serde_json::json!({}),
            terminate: None,
        }));
        let ctx = dummy_context(vec![tool]);
        let calls = vec![dummy_tool_call("1", "t1", serde_json::json!({}))];
        let msg = dummy_assistant_message();

        let batch = execute_tool_calls(
            &ctx, &msg, &calls,
            ToolExecutionMode::Sequential, &None, &None, &None, &dummy_event_sink(),
        )
        .await;

        assert_eq!(batch.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_dispatcher_tool_level_sequential_forces_sequential() {
        // Tool with execution_mode = Sequential should force sequential even if mode is Parallel
        let seq_tool = Arc::new(AgentTool {
            name: "seq".to_string(),
            description: String::new(),
            label: "seq".to_string(),
            parameters_schema: serde_json::json!({}),
            execution_mode: Some(ToolExecutionMode::Sequential),
            prepare_arguments: None,
            execute: Arc::new(|_id, _args, _signal, _on_update| {
                Box::pin(async {
                    Ok(AgentToolResult {
                        content: vec![],
                        details: serde_json::json!({}),
                        terminate: None,
                    })
                })
            }),
        });
        let ctx = dummy_context(vec![seq_tool]);
        let calls = vec![dummy_tool_call("1", "seq", serde_json::json!({}))];
        let msg = dummy_assistant_message();

        // Even with parallel mode, the tool's own execution mode should force sequential
        let batch = execute_tool_calls(
            &ctx, &msg, &calls,
            ToolExecutionMode::Parallel, &None, &None, &None, &dummy_event_sink(),
        )
        .await;

        assert_eq!(batch.messages.len(), 1);
    }
}