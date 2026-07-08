//! Common test helpers for pi-agent-core tests.
//!
//! Provides faux stream infrastructure, calculate tool, and message builders
//! matching the patterns in the original TypeScript tests.

use std::sync::Arc;

use futures::Stream;
use pi_agent_core::pi_ai_types::{
    AssistantMessage, AssistantMessageEvent, ContentBlock, Model, ModelCost, StopReason, Usage,
};
use pi_agent_core::types::{
    AgentMessage, AgentTool, AgentToolResult, StreamFn, StreamFnOptions,
};
use tokio::sync::mpsc;

// ============================================================================
// Message Builders
// ============================================================================

pub fn make_assistant(text: &str, stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            text_signature: None,
        }],
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        model: "mock".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        error_message: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

pub fn make_assistant_with_blocks(
    content: Vec<ContentBlock>,
    stop_reason: StopReason,
) -> AssistantMessage {
    AssistantMessage {
        content,
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        model: "mock".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        error_message: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

pub fn make_tool_call_block(id: &str, name: &str, args: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: args,
        thought_signature: None,
    }
}

pub fn make_text_block(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_string(),
        text_signature: None,
    }
}

pub fn make_thinking_block(text: &str) -> ContentBlock {
    ContentBlock::Thinking {
        thinking: text.to_string(),
        thinking_signature: None,
        redacted: None,
    }
}

pub fn make_user_message(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            text_signature: None,
        }],
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

pub fn make_assistant_message(text: &str) -> AgentMessage {
    AgentMessage::Assistant {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            text_signature: None,
        }],
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        model: "mock".to_string(),
        usage: Usage::default(),
        stop_reason: Some(StopReason::Stop),
        error_message: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

pub fn make_tool_result_message(
    tool_call_id: &str,
    tool_name: &str,
    text: &str,
) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            text_signature: None,
        }],
        details: serde_json::Value::Object(Default::default()),
        is_error: false,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

pub fn get_text_content(message: &AgentMessage) -> String {
    match message {
        AgentMessage::Assistant { content, .. } | AgentMessage::ToolResult { content, .. } => {
            content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        AgentMessage::User { content, .. } => content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ============================================================================
// Mock Model
// ============================================================================

pub fn mock_model() -> Model {
    Model {
        id: "mock".to_string(),
        name: "mock".to_string(),
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        base_url: "https://example.invalid".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".to_string()],
        cost: ModelCost::default(),
        context_window: 8192,
        max_tokens: 2048,
        headers: None,
        compat: None,
    }
}

pub fn mock_reasoning_model() -> Model {
    let mut m = mock_model();
    m.id = "mock-reasoning".to_string();
    m.reasoning = true;
    m
}

// ============================================================================
// Calculate Tool
// ============================================================================

pub fn make_calculate_tool() -> Arc<pi_agent_core::types::DynTool> {
    Arc::new(AgentTool {
        name: "calculate".to_string(),
        description: "Evaluate mathematical expressions".to_string(),
        label: "Calculator".to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "The mathematical expression to evaluate"
                }
            },
            "required": ["expression"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(|_id, args: serde_json::Value, _signal, _on_update| {
            Box::pin(async move {
                let expression = args
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                // Simple evaluation for test expressions
                let result = evaluate_expression(expression);
                Ok(AgentToolResult {
                    content: vec![ContentBlock::Text {
                        text: format!("{} = {}", expression, result),
                        text_signature: None,
                    }],
                    details: serde_json::Value::Object(Default::default()),
                    terminate: None,
                })
            })
        }),
    })
}

fn evaluate_expression(expr: &str) -> i64 {
    // Simple evaluator for common test expressions
    let expr = expr.trim();
    if let Some((a, b)) = expr.split_once('+') {
        let a: i64 = a.trim().parse().unwrap_or(0);
        let b: i64 = b.trim().parse().unwrap_or(0);
        a + b
    } else if let Some((a, b)) = expr.split_once('*') {
        let a: i64 = a.trim().parse().unwrap_or(0);
        let b: i64 = b.trim().parse().unwrap_or(0);
        a * b
    } else if let Some((a, b)) = expr.split_once('-') {
        let a: i64 = a.trim().parse().unwrap_or(0);
        let b: i64 = b.trim().parse().unwrap_or(0);
        a - b
    } else if let Some((a, b)) = expr.split_once('/') {
        let a: i64 = a.trim().parse().unwrap_or(0);
        let b: i64 = b.trim().parse().unwrap_or(0);
        if b == 0 { 0 } else { a / b }
    } else {
        expr.parse().unwrap_or(0)
    }
}

// ============================================================================
// Stream Fn Builder
// ============================================================================

/// Create a stream function that returns events from a channel.
pub fn make_stream_fn_from_rx(
    rx: mpsc::UnboundedReceiver<AssistantMessageEvent>,
) -> StreamFn {
    let rx = Arc::new(tokio::sync::Mutex::new(Some(rx)));
    Arc::new(move |_model, _ctx, _thinking, _opts| {
        let rx = rx.clone();
        Box::pin(async move {
            let rx_opt = rx.lock().await.take();
            match rx_opt {
                Some(rx) => Ok(Box::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                    as Box<dyn Stream<Item = AssistantMessageEvent> + Send + Unpin>),
                None => Err("Stream already consumed".into()),
            }
        })
    })
}

/// Create a stream function that returns a single assistant message.
pub fn make_single_response_stream_fn(text: &str) -> StreamFn {
    let (tx, rx) = mpsc::unbounded_channel();
    let msg = make_assistant(text, StopReason::Stop);
    tx.send(AssistantMessageEvent::Start {
        partial: msg.clone(),
    })
    .ok();
    tx.send(AssistantMessageEvent::TextDelta {
        content_index: 0,
        delta: text.to_string(),
        partial: msg.clone(),
    })
    .ok();
    tx.send(AssistantMessageEvent::Done {
        reason: StopReason::Stop,
        message: msg,
    })
    .ok();

    let rx = Arc::new(tokio::sync::Mutex::new(Some(rx)));
    Arc::new(move |_model, _ctx, _thinking, _opts| {
        let rx = rx.clone();
        Box::pin(async move {
            let rx_opt = rx.lock().await.take();
            match rx_opt {
                Some(rx) => Ok(Box::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                    as Box<dyn Stream<Item = AssistantMessageEvent> + Send + Unpin>),
                None => Err("Stream already consumed".into()),
            }
        })
    })
}

/// Create a stream function that returns a tool use response followed by a text response.
pub fn make_tool_response_stream_fn(
    tool_blocks: Vec<ContentBlock>,
    follow_up_text: &str,
) -> StreamFn {
    let (tx, rx) = mpsc::unbounded_channel();

    let tool_msg = make_assistant_with_blocks(tool_blocks, StopReason::ToolUse);
    tx.send(AssistantMessageEvent::Start {
        partial: tool_msg.clone(),
    })
    .ok();
    tx.send(AssistantMessageEvent::Done {
        reason: StopReason::ToolUse,
        message: tool_msg,
    })
    .ok();

    let follow_up = make_assistant(follow_up_text, StopReason::Stop);
    tx.send(AssistantMessageEvent::Start {
        partial: follow_up.clone(),
    })
    .ok();
    tx.send(AssistantMessageEvent::Done {
        reason: StopReason::Stop,
        message: follow_up,
    })
    .ok();

    let rx = Arc::new(tokio::sync::Mutex::new(Some(rx)));
    Arc::new(move |_model, _ctx, _thinking, _opts| {
        let rx = rx.clone();
        Box::pin(async move {
            let rx_opt = rx.lock().await.take();
            match rx_opt {
                Some(rx) => Ok(Box::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                    as Box<dyn Stream<Item = AssistantMessageEvent> + Send + Unpin>),
                None => Err("Stream already consumed".into()),
            }
        })
    })
}

/// Create a stream function that returns a slow stream (for abort tests).
pub fn make_slow_stream_fn(
    tokens: &[&str],
    delay_ms: u64,
) -> (StreamFn, Arc<std::sync::atomic::AtomicBool>) {
    let aborted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let aborted_clone = aborted.clone();
    let tokens: Vec<String> = tokens.iter().map(|s| s.to_string()).collect();

    let stream_fn: StreamFn = Arc::new(move |_model, _ctx, _thinking, opts: StreamFnOptions| {
        let tokens = tokens.clone();
        let aborted = aborted_clone.clone();
        let signal = opts.signal.clone();
        Box::pin(async move {
            let (tx, rx) = mpsc::unbounded_channel();
            let tx_clone = tx.clone();

            tokio::spawn(async move {
                let full_text = tokens.join(" ");
                let msg = make_assistant(&full_text, StopReason::Stop);
                tx_clone
                    .send(AssistantMessageEvent::Start {
                        partial: msg.clone(),
                    })
                    .ok();

                for token in &tokens {
                    // Check if aborted
                    if let Some(ref sig) = signal {
                        if *sig.borrow() {
                            aborted.store(true, std::sync::atomic::Ordering::SeqCst);
                            tx_clone
                                .send(AssistantMessageEvent::Error {
                                    reason: StopReason::Aborted,
                                    error: make_assistant("Aborted", StopReason::Aborted),
                                })
                                .ok();
                            return;
                        }
                    }

                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

                    tx_clone
                        .send(AssistantMessageEvent::TextDelta {
                            content_index: 0,
                            delta: token.clone(),
                            partial: msg.clone(),
                        })
                        .ok();
                }

                tx_clone
                    .send(AssistantMessageEvent::Done {
                        reason: StopReason::Stop,
                        message: msg,
                    })
                    .ok();
            });

            Ok(Box::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                as Box<dyn Stream<Item = AssistantMessageEvent> + Send + Unpin>)
        })
    });

    (stream_fn, aborted)
}
