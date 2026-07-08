//! End-to-end tests for Agent, ported from the original TypeScript e2e.test.ts.
//!
//! Tests basic prompt, tool execution, abort, lifecycle events, multi-turn,
//! thinking blocks, and continue() behavior.

mod common;

use std::sync::Arc;

use futures::Stream;
use pi_agent_core::agent::{Agent, AgentOptions, PromptInput};
use pi_agent_core::pi_ai_types::{
    AssistantMessageEvent, ContentBlock, StopReason,
};
use pi_agent_core::types::{AgentMessage, AgentState, ConvertToLlmFn, StreamFn};
use tokio::sync::mpsc;

use common::*;

fn default_convert_to_llm() -> ConvertToLlmFn {
    Arc::new(|messages: &[AgentMessage]| {
        messages
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    AgentMessage::User { .. }
                        | AgentMessage::Assistant { .. }
                        | AgentMessage::ToolResult { .. }
                )
            })
            .map(|m| match m {
                AgentMessage::User { content, timestamp } => {
                    pi_agent_core::pi_ai_types::Message::User {
                        content: content.clone(),
                        timestamp: *timestamp,
                    }
                }
                AgentMessage::Assistant {
                    content,
                    api,
                    provider,
                    model,
                    usage,
                    stop_reason,
                    error_message,
                    timestamp,
                } => pi_agent_core::pi_ai_types::Message::Assistant {
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
                },
                AgentMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    details,
                    is_error,
                    timestamp,
                } => pi_agent_core::pi_ai_types::Message::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    content: content.clone(),
                    details: Some(details.clone()),
                    is_error: *is_error,
                    timestamp: *timestamp,
                },
                _ => unreachable!(),
            })
            .collect()
    })
}

// ============================================================================
// Basic Prompt
// ============================================================================

#[tokio::test]
async fn test_basic_text_prompt() {
    let stream_fn = make_single_response_stream_fn("4");
    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant. Keep your responses concise.".to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    let result = agent
        .prompt(PromptInput::Text("What is 2+2? Answer with just the number."))
        .await
        .unwrap();

    let state = agent.state().await;
    assert!(!state.is_streaming);
    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[0].role(), "user");
    assert_eq!(state.messages[1].role(), "assistant");
    assert!(get_text_content(&state.messages[1]).contains("4"));
}

// ============================================================================
// Tool Execution
// ============================================================================

#[tokio::test]
async fn test_tool_execution() {
    let response_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let response_count_clone = response_count.clone();

    let stream_fn: StreamFn = Arc::new(move |_model, _ctx, _thinking, _opts| {
        let count = response_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tx, rx): (mpsc::UnboundedSender<AssistantMessageEvent>, _) = mpsc::unbounded_channel();

        if count == 0 {
            // First response: tool use
            let msg = make_assistant_with_blocks(
                vec![
                    make_text_block("Let me calculate that."),
                    make_tool_call_block(
                        "calc-1",
                        "calculate",
                        serde_json::json!({"expression": "123 * 456"}),
                    ),
                ],
                StopReason::ToolUse,
            );
            tx.send(AssistantMessageEvent::Start {
                partial: msg.clone(),
            })
            .ok();
            tx.send(AssistantMessageEvent::Done {
                reason: StopReason::ToolUse,
                message: msg,
            })
            .ok();
        } else {
            // Second response: follow-up text
            let msg = make_assistant("The result is 56088.", StopReason::Stop);
            tx.send(AssistantMessageEvent::Start {
                partial: msg.clone(),
            })
            .ok();
            tx.send(AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                message: msg,
            })
            .ok();
        }

        Box::pin(async move {
            Ok(Box::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                as Box<dyn Stream<Item = AssistantMessageEvent> + Send + Unpin>)
        })
    });

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant. Always use the calculator tool for math."
                .to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![make_calculate_tool()],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    let result = agent
        .prompt(PromptInput::Text(
            "Calculate 123 * 456 using the calculator tool.",
        ))
        .await
        .unwrap();

    let state = agent.state().await;
    assert!(!state.is_streaming);
    assert!(state.messages.len() >= 4);

    let tool_result_msg = state
        .messages
        .iter()
        .find(|m| m.role() == "toolResult");
    assert!(tool_result_msg.is_some());
    assert!(get_text_content(tool_result_msg.unwrap()).contains("123 * 456 = 56088"));

    let final_message = state.messages.last().unwrap();
    assert_eq!(final_message.role(), "assistant");
    assert!(get_text_content(final_message).contains("56088"));
    assert_eq!(state.pending_tool_calls.len(), 0);
}

// ============================================================================
// Abort During Streaming
// ============================================================================

#[tokio::test]
async fn test_abort_during_streaming() {
    let (stream_fn, _aborted) = make_slow_stream_fn(
        &["one", "two", "three", "four", "five"],
        50, // 50ms per token
    );

    let agent = Arc::new(Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant.".to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    }));

    let agent_clone = agent.clone();
    let prompt_handle = tokio::spawn(async move {
        agent_clone
            .prompt(PromptInput::Text("Count slowly from 1 to 5."))
            .await
    });

    // Wait a bit then abort
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    agent.abort().await;

    let result = prompt_handle.await.unwrap();
    // Should succeed with an aborted message
    assert!(result.is_ok() || result.is_err() == false);

    let state = agent.state().await;
    assert!(!state.is_streaming);
    assert!(state.messages.len() >= 2);

    let last_message = state.messages.last().unwrap();
    assert_eq!(last_message.role(), "assistant");
}

// ============================================================================
// Lifecycle Events
// ============================================================================

#[tokio::test]
async fn test_lifecycle_events() {
    let stream_fn = make_single_response_stream_fn("1 2 3 4 5");
    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant.".to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let events_clone = events.clone();

    agent
        .subscribe(Arc::new(move |event, _signal| {
            let events = events_clone.clone();
            Box::pin(async move {
                let type_str = match &event {
                    pi_agent_core::types::AgentEvent::AgentStart => "agent_start",
                    pi_agent_core::types::AgentEvent::AgentEnd { .. } => "agent_end",
                    pi_agent_core::types::AgentEvent::TurnStart => "turn_start",
                    pi_agent_core::types::AgentEvent::TurnEnd { .. } => "turn_end",
                    pi_agent_core::types::AgentEvent::MessageStart { .. } => "message_start",
                    pi_agent_core::types::AgentEvent::MessageUpdate { .. } => "message_update",
                    pi_agent_core::types::AgentEvent::MessageEnd { .. } => "message_end",
                    pi_agent_core::types::AgentEvent::ToolExecutionStart { .. } => {
                        "tool_execution_start"
                    }
                    pi_agent_core::types::AgentEvent::ToolExecutionUpdate { .. } => {
                        "tool_execution_update"
                    }
                    pi_agent_core::types::AgentEvent::ToolExecutionEnd { .. } => {
                        "tool_execution_end"
                    }
                };
                events.lock().unwrap().push(type_str.to_string());
            })
        }))
        .await;

    agent
        .prompt(PromptInput::Text("Count from 1 to 5."))
        .await
        .unwrap();

    let events = events.lock().unwrap();
    assert!(
        !events.is_empty(),
        "No events received at all"
    );
    assert!(
        events.contains(&"agent_start".to_string()),
        "Events did not contain agent_start. All events: {:?}",
        events
    );
    assert!(events.contains(&"turn_start".to_string()));
    assert!(events.contains(&"message_start".to_string()));
    assert!(events.contains(&"message_end".to_string()));
    assert!(events.contains(&"turn_end".to_string()));
    assert!(events.contains(&"agent_end".to_string()));

    let agent_start_idx = events.iter().position(|e| e == "agent_start").unwrap();
    let msg_start_idx = events.iter().position(|e| e == "message_start").unwrap();
    let msg_end_idx = events.iter().position(|e| e == "message_end").unwrap();
    let agent_end_idx = events.iter().rposition(|e| e == "agent_end").unwrap();

    assert!(agent_start_idx < msg_start_idx);
    assert!(msg_start_idx < msg_end_idx);
    assert!(msg_end_idx < agent_end_idx);

    let state = agent.state().await;
    assert!(!state.is_streaming);
    assert_eq!(state.messages.len(), 2);
}

// ============================================================================
// Multi-turn Conversation
// ============================================================================

#[tokio::test]
async fn test_multi_turn_conversation() {
    let response_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let response_count_clone = response_count.clone();

    let stream_fn: StreamFn = Arc::new(move |_model, ctx, _thinking, _opts| {
        let count = response_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tx, rx): (mpsc::UnboundedSender<AssistantMessageEvent>, _) = mpsc::unbounded_channel();

        let msg = if count == 0 {
            make_assistant("Nice to meet you, Alice.", StopReason::Stop)
        } else {
            // Check if context has "Alice"
            let has_alice = ctx.messages.iter().any(|m| {
                if let pi_agent_core::pi_ai_types::Message::User { content, .. } = m {
                    content.iter().any(|b| {
                        if let ContentBlock::Text { text, .. } = b {
                            text.contains("Alice")
                        } else {
                            false
                        }
                    })
                } else {
                    false
                }
            });
            if has_alice {
                make_assistant("Your name is Alice.", StopReason::Stop)
            } else {
                make_assistant("I do not know your name.", StopReason::Stop)
            }
        };

        tx.send(AssistantMessageEvent::Start {
            partial: msg.clone(),
        })
        .ok();
        tx.send(AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: msg,
        })
        .ok();

        Box::pin(async move {
            Ok(Box::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                as Box<dyn Stream<Item = AssistantMessageEvent> + Send + Unpin>)
        })
    });

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant.".to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    agent
        .prompt(PromptInput::Text("My name is Alice."))
        .await
        .unwrap();
    assert_eq!(agent.state().await.messages.len(), 2);

    agent
        .prompt(PromptInput::Text("What is my name?"))
        .await
        .unwrap();
    assert_eq!(agent.state().await.messages.len(), 4);

    let state = agent.state().await;
    let last_message = &state.messages[3];
    assert_eq!(last_message.role(), "assistant");
    assert!(
        get_text_content(last_message).to_lowercase().contains("alice"),
        "Expected response to contain 'alice', got: {}",
        get_text_content(last_message)
    );
}

// ============================================================================
// Thinking Content Blocks
// ============================================================================

#[tokio::test]
async fn test_thinking_content_blocks() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    let msg = make_assistant_with_blocks(
        vec![
            make_thinking_block("step by step"),
            make_text_block("4"),
        ],
        StopReason::Stop,
    );
    tx.send(AssistantMessageEvent::Start {
        partial: msg.clone(),
    })
    .ok();
    tx.send(AssistantMessageEvent::Done {
        reason: StopReason::Stop,
        message: msg,
    })
    .ok();

    let stream_fn = make_stream_fn_from_rx(rx);

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant.".to_string(),
            model: mock_reasoning_model(),
            thinking_level: "low".to_string(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    agent
        .prompt(PromptInput::Text("What is 2+2?"))
        .await
        .unwrap();

    let state = agent.state().await;
    let assistant_msg = &state.messages[1];
    assert_eq!(assistant_msg.role(), "assistant");

    if let AgentMessage::Assistant { content, .. } = assistant_msg {
        assert_eq!(content.len(), 2);
        assert!(matches!(content[0], ContentBlock::Thinking { .. }));
        assert!(matches!(content[1], ContentBlock::Text { .. }));
    } else {
        panic!("Expected assistant message");
    }
}

// ============================================================================
// Continue() Tests
// ============================================================================

#[tokio::test]
async fn test_continue_no_messages() {
    let stream_fn = make_single_response_stream_fn("HELLO WORLD");
    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "Test".to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    let result = agent.continue_run().await;
    assert!(result.is_err(), "Expected error, got Ok");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("No messages") || err.contains("no messages"),
        "Expected error about no messages, got: {}",
        err
    );
}

#[tokio::test]
async fn test_continue_last_message_assistant() {
    let stream_fn = make_single_response_stream_fn("Hello");
    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "Test".to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![],
            messages: vec![make_assistant_message("Hello")],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    let result = agent.continue_run().await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Cannot continue from message role: assistant"));
}

#[tokio::test]
async fn test_continue_from_user_message() {
    let stream_fn = make_single_response_stream_fn("HELLO WORLD");
    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant. Follow instructions exactly."
                .to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![],
            messages: vec![make_user_message("Say exactly: HELLO WORLD")],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    agent.continue_run().await.unwrap();

    let state = agent.state().await;
    assert!(!state.is_streaming);
    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[0].role(), "user");
    assert_eq!(state.messages[1].role(), "assistant");
    assert!(get_text_content(&state.messages[1])
        .to_uppercase()
        .contains("HELLO WORLD"));
}

#[tokio::test]
async fn test_continue_from_tool_result() {
    let stream_fn = make_single_response_stream_fn("The answer is 8.");
    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant. After getting a calculation result, state the answer clearly."
                .to_string(),
            model: mock_model(),
            thinking_level: "off".to_string(),
            tools: vec![make_calculate_tool()],
            messages: vec![
                make_user_message("What is 5 + 3?"),
                AgentMessage::Assistant {
                    content: vec![
                        make_text_block("Let me calculate that."),
                        make_tool_call_block("calc-1", "calculate", serde_json::json!({"expression": "5 + 3"})),
                    ],
                    api: "openai-responses".to_string(),
                    provider: "openai".to_string(),
                    model: "mock".to_string(),
                    usage: Default::default(),
                    stop_reason: Some(StopReason::ToolUse),
                    error_message: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
                make_tool_result_message("calc-1", "calculate", "5 + 3 = 8"),
            ],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(default_convert_to_llm()),
        ..Default::default()
    });

    agent.continue_run().await.unwrap();

    let state = agent.state().await;
    assert!(!state.is_streaming);
    assert!(state.messages.len() >= 4);

    let last_message = state.messages.last().unwrap();
    assert_eq!(last_message.role(), "assistant");
    assert!(get_text_content(last_message).contains("8"));
}
