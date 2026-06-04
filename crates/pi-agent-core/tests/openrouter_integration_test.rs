//! Integration tests for pi-agent-core with OpenRouter.
//!
//! Verifies end-to-end Agent → pi-ai → OpenRouter connectivity.
//! Requires `OPENROUTER_API_KEY`. Uses a free model.
//!
//! Run with: `cargo test --test openrouter_integration_test -- --ignored --nocapture`

use pi_agent_core::agent::{Agent, AgentOptions};
use pi_agent_core::pi_ai_types::{
    AssistantMessageEvent, ContentBlock, Context, Message, Model, ModelCost, StopReason,
    ThinkingLevel,
};
use pi_agent_core::types::{AgentEvent, AgentMessage, AgentState, ConvertToLlmFn, StreamFn};
use pi_ai::providers::register_builtins::register_built_in_api_providers;
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn test_model_id() -> String {
    env::var("PI_TEST_MODEL").unwrap_or_else(|_| "poolside/laguna-m.1:free".to_string())
}

fn require_api_key() -> String {
    env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY must be set")
}

fn make_model() -> Model {
    let id = test_model_id();
    Model {
        id: id.clone(),
        name: format!("Test: {}", id),
        api: "openai-completions".to_string(),
        provider: "openrouter".to_string(),
        base_url: "https://openrouter.ai/api/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".to_string()],
        cost: ModelCost::default(),
        context_window: 128000,
        max_tokens: 4096,
        headers: None,
        compat: None,
    }
}

// ============================================================================
// StreamFn builder — bridges pi-agent-core → pi-ai
// ============================================================================

fn make_openrouter_stream_fn(api_key: &str) -> StreamFn {
    let key = api_key.to_string();
    Arc::new(
        move |model: Model,
              context: Context,
              _thinking_level: Option<ThinkingLevel>,
              _options: pi_agent_core::types::StreamFnOptions|
              -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            pi_agent_core::pi_ai_types::StreamResponse,
                            Box<dyn std::error::Error + Send + Sync>,
                        >,
                    > + Send,
            >,
        > {
            let key = key.clone();
            Box::pin(async move {
                let pi_model = pi_ai::types::Model {
                    id: model.id,
                    name: model.name,
                    api: model.api,
                    provider: model.provider,
                    base_url: model.base_url,
                    reasoning: model.reasoning,
                    thinking_level_map: model.thinking_level_map,
                    input: model.input,
                    cost: pi_ai::types::ModelCost {
                        input: model.cost.input,
                        output: model.cost.output,
                        cache_read: model.cost.cache_read,
                        cache_write: model.cost.cache_write,
                    },
                    context_window: model.context_window,
                    max_tokens: model.max_tokens,
                    headers: model.headers,
                    compat: None,
                };

                let pi_context = pi_ai::types::Context {
                    system_prompt: context.system_prompt,
                    messages: context
                        .messages
                        .iter()
                        .map(|m| match m {
                            Message::User { content, timestamp } => pi_ai::types::Message::User {
                                content: content
                                    .iter()
                                    .map(|cb| match cb {
                                        ContentBlock::Text { text, .. } => {
                                            pi_ai::types::ContentBlock::Text {
                                                text: text.clone(),
                                                text_signature: None,
                                            }
                                        }
                                        _ => pi_ai::types::ContentBlock::Text {
                                            text: String::new(),
                                            text_signature: None,
                                        },
                                    })
                                    .collect(),
                                timestamp: *timestamp,
                            },
                            _ => pi_ai::types::Message::User {
                                content: vec![],
                                timestamp: 0,
                            },
                        })
                        .collect(),
                    tools: None,
                };

                let stream_opts = pi_ai::types::StreamOptions {
                    api_key: Some(key),
                    ..Default::default()
                };

                let event_stream = pi_ai::stream::stream(&pi_model, &pi_context, Some(stream_opts));

                use futures::StreamExt;
                let converted: pi_agent_core::pi_ai_types::StreamResponse =
                    Box::new(event_stream.map(|event| match event {
                        pi_ai::types::AssistantMessageEvent::Start { partial } => {
                            AssistantMessageEvent::Start {
                                partial: convert_msg(partial),
                            }
                        }
                        pi_ai::types::AssistantMessageEvent::TextStart {
                            content_index,
                            partial,
                        } => AssistantMessageEvent::TextStart {
                            content_index,
                            partial: convert_msg(partial),
                        },
                        pi_ai::types::AssistantMessageEvent::TextDelta {
                            content_index,
                            delta,
                            partial,
                        } => AssistantMessageEvent::TextDelta {
                            content_index,
                            delta,
                            partial: convert_msg(partial),
                        },
                        pi_ai::types::AssistantMessageEvent::TextEnd {
                            content_index,
                            content,
                            partial,
                        } => AssistantMessageEvent::TextEnd {
                            content_index,
                            content,
                            partial: convert_msg(partial),
                        },
                        pi_ai::types::AssistantMessageEvent::Done { reason, message } => {
                            AssistantMessageEvent::Done {
                                reason: match reason {
                                    pi_ai::types::StopReason::Stop => StopReason::Stop,
                                    pi_ai::types::StopReason::ToolUse => StopReason::ToolUse,
                                    pi_ai::types::StopReason::Aborted => StopReason::Aborted,
                                    _ => StopReason::Error,
                                },
                                message: convert_msg(message),
                            }
                        }
                        pi_ai::types::AssistantMessageEvent::Error { reason: _, error } => {
                            AssistantMessageEvent::Error {
                                reason: StopReason::Error,
                                error: convert_msg(error),
                            }
                        }
                        other => {
                            // Map remaining event types through
                            match other {
                                pi_ai::types::AssistantMessageEvent::ThinkingStart {
                                    content_index,
                                    partial,
                                } => AssistantMessageEvent::ThinkingStart {
                                    content_index,
                                    partial: convert_msg(partial),
                                },
                                pi_ai::types::AssistantMessageEvent::ThinkingDelta {
                                    content_index,
                                    delta,
                                    partial,
                                } => AssistantMessageEvent::ThinkingDelta {
                                    content_index,
                                    delta,
                                    partial: convert_msg(partial),
                                },
                                pi_ai::types::AssistantMessageEvent::ThinkingEnd {
                                    content_index,
                                    content,
                                    partial,
                                } => AssistantMessageEvent::ThinkingEnd {
                                    content_index,
                                    content,
                                    partial: convert_msg(partial),
                                },
                                pi_ai::types::AssistantMessageEvent::ToolCallStart {
                                    content_index,
                                    partial,
                                } => AssistantMessageEvent::ToolCallStart {
                                    content_index,
                                    partial: convert_msg(partial),
                                },
                                pi_ai::types::AssistantMessageEvent::ToolCallDelta {
                                    content_index,
                                    delta,
                                    partial,
                                } => AssistantMessageEvent::ToolCallDelta {
                                    content_index,
                                    delta,
                                    partial: convert_msg(partial),
                                },
                                pi_ai::types::AssistantMessageEvent::ToolCallEnd {
                                    content_index,
                                    tool_call,
                                    partial,
                                } => AssistantMessageEvent::ToolCallEnd {
                                    content_index,
                                    tool_call: pi_agent_core::pi_ai_types::ToolCall {
                                        type_field: "toolCall".to_string(),
                                        id: tool_call.id,
                                        name: tool_call.name,
                                        arguments: tool_call.arguments,
                                        thought_signature: None,
                                    },
                                    partial: convert_msg(partial),
                                },
                                _ => AssistantMessageEvent::Error {
                                    reason: StopReason::Error,
                                    error: convert_msg(pi_ai::types::AssistantMessage {
                                        content: vec![],
                                        api: String::new(),
                                        provider: String::new(),
                                        model: String::new(),
                                        response_model: None,
                                        response_id: None,
                                        diagnostics: None,
                                        usage: pi_ai::types::Usage::default(),
                                        stop_reason: pi_ai::types::StopReason::Error,
                                        error_message: Some("Unknown event".into()),
                                        timestamp: 0,
                                    }),
                                },
                            }
                        }
                    }));

                Ok(converted)
            })
        },
    )
}

fn convert_msg(
    msg: pi_ai::types::AssistantMessage,
) -> pi_agent_core::pi_ai_types::AssistantMessage {
    pi_agent_core::pi_ai_types::AssistantMessage {
        content: msg
            .content
            .iter()
            .map(|cb| match cb {
                pi_ai::types::ContentBlock::Text {
                    text,
                    text_signature,
                } => ContentBlock::Text {
                    text: text.clone(),
                    text_signature: text_signature.clone(),
                },
                pi_ai::types::ContentBlock::Thinking {
                    thinking,
                    thinking_signature,
                    redacted,
                } => ContentBlock::Thinking {
                    thinking: thinking.clone(),
                    thinking_signature: thinking_signature.clone(),
                    redacted: *redacted,
                },
                pi_ai::types::ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    thought_signature,
                } => ContentBlock::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    thought_signature: thought_signature.clone(),
                },
                pi_ai::types::ContentBlock::Image { data, mime_type } => ContentBlock::Image {
                    data: data.clone(),
                    mime_type: mime_type.clone(),
                },
            })
            .collect(),
        api: msg.api,
        provider: msg.provider,
        model: msg.model,
        response_model: msg.response_model,
        response_id: msg.response_id,
        diagnostics: msg.diagnostics,
        usage: msg.usage,
        stop_reason: msg.stop_reason,
        error_message: msg.error_message,
        timestamp: msg.timestamp,
    }
}

// ============================================================================
// Helper: ConvertToLlmFn
// ============================================================================

fn make_convert_to_llm() -> ConvertToLlmFn {
    Arc::new(|messages: &[AgentMessage]| {
        messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::User { content, timestamp } => Some(Message::User {
                    content: content.clone(),
                    timestamp: *timestamp,
                }),
                AgentMessage::Assistant {
                    content,
                    api,
                    provider,
                    model,
                    usage,
                    stop_reason,
                    error_message,
                    timestamp,
                } => Some(Message::Assistant {
                    content: content.clone(),
                    api: api.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: usage.clone(),
                    stop_reason: stop_reason.clone().unwrap_or(StopReason::Stop),
                    error_message: error_message.clone(),
                    timestamp: *timestamp,
                }),
                AgentMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    details,
                    is_error,
                    timestamp,
                } => Some(Message::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    content: content.clone(),
                    details: Some(details.clone()),
                    is_error: *is_error,
                    timestamp: *timestamp,
                }),
                _ => None,
            })
            .collect()
    })
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_agent_basic_prompt() {
    register_built_in_api_providers();
    let api_key = require_api_key();
    let model = make_model();

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "Reply with EXACTLY the word 'OK' and nothing else.".into(),
            model,
            thinking_level: "off".into(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        convert_to_llm: Some(make_convert_to_llm()),
        stream_fn: Some(make_openrouter_stream_fn(&api_key)),
        ..Default::default()
    });

    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let e = events.clone();
    let _handle = agent
        .subscribe(Arc::new(move |event, _| {
            let e = e.clone();
            Box::pin(async move {
                e.lock().unwrap().push(format!("{:?}", event));
            })
        }))
        .await;

    let result = agent
        .process(vec![AgentMessage::User {
            content: vec![ContentBlock::Text {
                text: "ping".to_string(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }])
        .await;

    match result {
        Ok(messages) => {
            println!("Got {} messages", messages.len());
            for msg in &messages {
                if let AgentMessage::Assistant { content, .. } = msg {
                    let text: String = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    println!("Response: {}", text);
                    assert!(!text.is_empty());
                }
            }
            assert!(!messages.is_empty());
        }
        Err(e) => panic!("Agent failed: {}", e),
    }

    let ev = events.lock().unwrap();
    println!("{} events received", ev.len());
    assert!(!ev.is_empty());
}

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_agent_multi_turn() {
    register_built_in_api_providers();
    let api_key = require_api_key();
    let model = make_model();

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "Answer in exactly one sentence.".into(),
            model,
            thinking_level: "off".into(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        convert_to_llm: Some(make_convert_to_llm()),
        stream_fn: Some(make_openrouter_stream_fn(&api_key)),
        ..Default::default()
    });

    // Turn 1
    let r1 = agent
        .process(vec![AgentMessage::User {
            content: vec![ContentBlock::Text {
                text: "What is 1+1?".into(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }])
        .await
        .expect("Turn 1 failed");

    let has_a1 = r1
        .iter()
        .any(|m| matches!(m, AgentMessage::Assistant { .. }));
    assert!(has_a1, "Turn 1 should have assistant response");
    // process() returns new messages, state.messages has user msg + r1
    assert!(!r1.is_empty(), "process should return messages");

    // Turn 2
    let r2 = agent
        .process(vec![AgentMessage::User {
            content: vec![ContentBlock::Text {
                text: "Multiply that by 3.".into(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }])
        .await;

    match r2 {
        Ok(msgs) => {
            let text: String = msgs
                .iter()
                .filter_map(|m| match m {
                    AgentMessage::Assistant { content, .. } => Some(
                        content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text, .. } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(""),
                    ),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            println!("Turn 2: {}", text);
            let lower = text.to_lowercase();
            assert!(
                lower.contains('6') || lower.contains("six"),
                "Should mention 6: {}",
                text
            );
        }
        Err(e) => panic!("Turn 2 failed: {}", e),
    }
}

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_agent_idle_state() {
    register_built_in_api_providers();
    let api_key = require_api_key();
    let model = make_model();

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "Reply with only 'hello' in lowercase.".into(),
            model,
            thinking_level: "off".into(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        convert_to_llm: Some(make_convert_to_llm()),
        stream_fn: Some(make_openrouter_stream_fn(&api_key)),
        ..Default::default()
    });

    assert!(!agent.state().await.is_streaming);

    let result = agent
        .process(vec![AgentMessage::User {
            content: vec![ContentBlock::Text {
                text: "say hi".into(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }])
        .await;

    assert!(result.is_ok());
    let state = agent.state().await;
    assert!(!state.is_streaming, "Agent should be idle after completion");
    assert!(state.error_message.is_none(), "Should have no error");
    println!(
        "State: model={}/{}, messages={}, thinking={}",
        state.model.provider,
        state.model.id,
        state.messages.len(),
        state.thinking_level
    );
}

// ============================================================================
// Streaming delta test
// ============================================================================

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_streaming_deltas() {
    register_built_in_api_providers();
    let api_key = require_api_key();
    let model = make_model();

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "Write a detailed paragraph about Rust programming (at least 50 words)."
                .into(),
            model,
            thinking_level: "off".into(),
            tools: vec![],
            messages: vec![],
            is_streaming: true,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        convert_to_llm: Some(make_convert_to_llm()),
        stream_fn: Some(make_openrouter_stream_fn(&api_key)),
        ..Default::default()
    });

    let text_delta_count = Arc::new(AtomicUsize::new(0));
    let count = text_delta_count.clone();

    let _handle = agent
        .subscribe(Arc::new(move |event, _| {
            let count = count.clone();
            Box::pin(async move {
                if let AgentEvent::MessageUpdate {
                    assistant_message_event,
                    ..
                } = &event
                {
                    if matches!(
                        assistant_message_event,
                        AssistantMessageEvent::TextDelta { .. }
                    ) {
                        count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        }))
        .await;

    let result = agent
        .process(vec![AgentMessage::User {
            content: vec![ContentBlock::Text {
                text: "Tell me about Rust's ownership system in 2-3 sentences.".into(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }])
        .await;

    assert!(result.is_ok(), "Agent process failed: {:?}", result.err());

    let n = text_delta_count.load(Ordering::Relaxed);
    println!("TextDelta events received: {}", n);
    assert!(
        n >= 3,
        "Expected at least 3 TextDelta events from streaming, got {}",
        n
    );
}
