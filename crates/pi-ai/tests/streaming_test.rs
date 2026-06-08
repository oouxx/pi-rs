//! E2E tests for pi-ai streaming.
//!
//! Tests the `AssistantMessageEventStream` API using mock event streams
//! that simulate real LLM streaming responses without network calls.
//!
//! Run: cargo test -p pi-ai --test streaming_test

use pi_ai::types::{
    AssistantMessage, AssistantMessageEvent, ContentBlock, StopReason, ToolCall, Usage,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;

use futures::stream;

// ── helpers ────────────────────────────────────────────────────────────────

fn partial_msg(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            text_signature: None,
        }],
        api: "mock".into(),
        provider: "mock".into(),
        model: "mock-model".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

fn final_msg(text: &str, stop: StopReason) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            text_signature: None,
        }],
        api: "mock".into(),
        provider: "mock".into(),
        model: "mock-model".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: stop,
        error_message: None,
        timestamp: 0,
    }
}

// ── Text streaming ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_stream_text_delta_accumulates() {
    let events = vec![
        AssistantMessageEvent::Start {
            partial: partial_msg(""),
        },
        AssistantMessageEvent::TextStart {
            content_index: 0,
            partial: partial_msg(""),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "Hel".into(),
            partial: partial_msg("Hel"),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "lo ".into(),
            partial: partial_msg("Hello "),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "world".into(),
            partial: partial_msg("Hello world"),
        },
        AssistantMessageEvent::TextEnd {
            content_index: 0,
            content: "Hello world".into(),
            partial: partial_msg("Hello world"),
        },
        AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: final_msg("Hello world", StopReason::Stop),
        },
    ];

    let stream = AssistantMessageEventStream::new(stream::iter(events));
    let msg = stream.result().await.unwrap();
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.content.len(), 1);
    match &msg.content[0] {
        ContentBlock::Text { text, .. } => assert_eq!(text, "Hello world"),
        _ => panic!("expected Text content block"),
    }
}

#[tokio::test]
async fn test_stream_preserves_final_text_content() {
    let final_text =
        "# Hello\n\nThis is a **markdown** response with `code`.\n\n- list item 1\n- list item 2";
    let deltas = [
        "# Hello\n\n",
        "This is a **markdown** ",
        "response with `code`.\n\n",
        "- list item 1\n",
        "- list item 2",
    ];

    let mut events: Vec<AssistantMessageEvent> = vec![
        AssistantMessageEvent::Start {
            partial: partial_msg(""),
        },
        AssistantMessageEvent::TextStart {
            content_index: 0,
            partial: partial_msg(""),
        },
    ];

    let mut accumulated = String::new();
    for delta in &deltas {
        accumulated.push_str(delta);
        events.push(AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: delta.to_string(),
            partial: partial_msg(&accumulated),
        });
    }

    events.push(AssistantMessageEvent::TextEnd {
        content_index: 0,
        content: accumulated.clone(),
        partial: partial_msg(&accumulated),
    });
    events.push(AssistantMessageEvent::Done {
        reason: StopReason::Stop,
        message: final_msg(&accumulated, StopReason::Stop),
    });

    let stream = AssistantMessageEventStream::new(stream::iter(events));
    let msg = stream.result().await.unwrap();
    match &msg.content[0] {
        ContentBlock::Text { text, .. } => assert_eq!(text, final_text),
        _ => panic!("expected Text content block"),
    }
}

// ── Thinking + Text multi-block streaming ──────────────────────────────────

#[tokio::test]
async fn test_stream_thinking_then_text() {
    let empty = || AssistantMessage {
        content: vec![],
        ..partial_msg("")
    };

    let events = vec![
        AssistantMessageEvent::Start { partial: empty() },
        AssistantMessageEvent::ThinkingStart {
            content_index: 0,
            partial: AssistantMessage {
                content: vec![ContentBlock::Thinking {
                    thinking: "".into(),
                    thinking_signature: None,
                    redacted: None,
                }],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "Let me think".into(),
            partial: AssistantMessage {
                content: vec![ContentBlock::Thinking {
                    thinking: "Let me think".into(),
                    thinking_signature: None,
                    redacted: None,
                }],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: " about this...".into(),
            partial: AssistantMessage {
                content: vec![ContentBlock::Thinking {
                    thinking: "Let me think about this...".into(),
                    thinking_signature: None,
                    redacted: None,
                }],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            content: "Let me think about this...".into(),
            partial: AssistantMessage {
                content: vec![ContentBlock::Thinking {
                    thinking: "Let me think about this...".into(),
                    thinking_signature: Some("sig123".into()),
                    redacted: None,
                }],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::TextStart {
            content_index: 1,
            partial: AssistantMessage {
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "Let me think about this...".into(),
                        thinking_signature: Some("sig123".into()),
                        redacted: None,
                    },
                    ContentBlock::Text {
                        text: "".into(),
                        text_signature: None,
                    },
                ],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::TextDelta {
            content_index: 1,
            delta: "Here is the ".into(),
            partial: AssistantMessage {
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "Let me think about this...".into(),
                        thinking_signature: Some("sig123".into()),
                        redacted: None,
                    },
                    ContentBlock::Text {
                        text: "Here is the ".into(),
                        text_signature: None,
                    },
                ],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::TextDelta {
            content_index: 1,
            delta: "answer.".into(),
            partial: AssistantMessage {
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "Let me think about this...".into(),
                        thinking_signature: Some("sig123".into()),
                        redacted: None,
                    },
                    ContentBlock::Text {
                        text: "Here is the answer.".into(),
                        text_signature: None,
                    },
                ],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::TextEnd {
            content_index: 1,
            content: "Here is the answer.".into(),
            partial: AssistantMessage {
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "Let me think about this...".into(),
                        thinking_signature: Some("sig123".into()),
                        redacted: None,
                    },
                    ContentBlock::Text {
                        text: "Here is the answer.".into(),
                        text_signature: None,
                    },
                ],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: AssistantMessage {
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "Let me think about this...".into(),
                        thinking_signature: Some("sig123".into()),
                        redacted: None,
                    },
                    ContentBlock::Text {
                        text: "Here is the answer.".into(),
                        text_signature: None,
                    },
                ],
                ..final_msg("", StopReason::Stop)
            },
        },
    ];

    let stream = AssistantMessageEventStream::new(stream::iter(events));
    let msg = stream.result().await.unwrap();
    assert_eq!(msg.content.len(), 2);
    match &msg.content[0] {
        ContentBlock::Thinking { thinking, .. } => {
            assert_eq!(thinking, "Let me think about this...");
        }
        _ => panic!("expected Thinking as first content block"),
    }
    match &msg.content[1] {
        ContentBlock::Text { text, .. } => {
            assert_eq!(text, "Here is the answer.");
        }
        _ => panic!("expected Text as second content block"),
    }
}

// ── Tool call streaming ────────────────────────────────────────────────────

#[tokio::test]
async fn test_stream_tool_call() {
    let events = vec![
        AssistantMessageEvent::Start {
            partial: AssistantMessage {
                content: vec![],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            partial: AssistantMessage {
                content: vec![],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{\"pa".into(),
            partial: AssistantMessage {
                content: vec![],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "th\": \"/tmp/x\"}".into(),
            partial: AssistantMessage {
                content: vec![],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ToolCallEnd {
            content_index: 0,
            tool_call: ToolCall::new(
                "call_1".into(),
                "read_file".into(),
                serde_json::json!({"path": "/tmp/x"}),
            ),
            partial: AssistantMessage {
                content: vec![ContentBlock::ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "/tmp/x"}),
                    thought_signature: None,
                }],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::Done {
            reason: StopReason::ToolUse,
            message: AssistantMessage {
                content: vec![ContentBlock::ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "/tmp/x"}),
                    thought_signature: None,
                }],
                stop_reason: StopReason::ToolUse,
                ..partial_msg("")
            },
        },
    ];

    let stream = AssistantMessageEventStream::new(stream::iter(events));
    let msg = stream.result().await.unwrap();
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.content.len(), 1);
    match &msg.content[0] {
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
            ..
        } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "read_file");
            assert_eq!(arguments, &serde_json::json!({"path": "/tmp/x"}));
        }
        _ => panic!("expected ToolCall content block"),
    }
}

// ── Error handling ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_stream_error_returns_error_message() {
    let err_msg = AssistantMessage {
        content: vec![],
        api: "mock".into(),
        provider: "mock".into(),
        model: "mock-model".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        error_message: Some("API rate limit exceeded".into()),
        timestamp: 0,
    };

    let events = vec![
        AssistantMessageEvent::Start {
            partial: partial_msg(""),
        },
        AssistantMessageEvent::Error {
            reason: StopReason::Error,
            error: err_msg.clone(),
        },
    ];

    let stream = AssistantMessageEventStream::new(stream::iter(events));
    let result = stream.result().await;
    assert!(result.is_ok(), "Error events should still return a message");
    let msg = result.unwrap();
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message, Some("API rate limit exceeded".into()));
}

#[tokio::test]
async fn test_stream_empty_returns_error() {
    let stream = AssistantMessageEventStream::new(stream::iter(vec![]));
    let result = stream.result().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("ended without a final event"));
}

// ── Interleaved text + tool call ───────────────────────────────────────────

#[tokio::test]
async fn test_stream_text_then_tool_call() {
    let events = vec![
        AssistantMessageEvent::Start {
            partial: partial_msg(""),
        },
        AssistantMessageEvent::TextStart {
            content_index: 0,
            partial: partial_msg(""),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "Let me search for that. ".into(),
            partial: partial_msg("Let me search for that. "),
        },
        AssistantMessageEvent::TextEnd {
            content_index: 0,
            content: "Let me search for that. ".into(),
            partial: partial_msg("Let me search for that. "),
        },
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            partial: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "Let me search for that. ".into(),
                    text_signature: None,
                }],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: "{\"que".into(),
            partial: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "Let me search for that. ".into(),
                    text_signature: None,
                }],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: "ry\": \"rust\"}".into(),
            partial: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "Let me search for that. ".into(),
                    text_signature: None,
                }],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::ToolCallEnd {
            content_index: 1,
            tool_call: ToolCall::new(
                "call_2".into(),
                "search".into(),
                serde_json::json!({"query": "rust"}),
            ),
            partial: AssistantMessage {
                content: vec![
                    ContentBlock::Text {
                        text: "Let me search for that. ".into(),
                        text_signature: None,
                    },
                    ContentBlock::ToolCall {
                        id: "call_2".into(),
                        name: "search".into(),
                        arguments: serde_json::json!({"query": "rust"}),
                        thought_signature: None,
                    },
                ],
                ..partial_msg("")
            },
        },
        AssistantMessageEvent::Done {
            reason: StopReason::ToolUse,
            message: AssistantMessage {
                content: vec![
                    ContentBlock::Text {
                        text: "Let me search for that. ".into(),
                        text_signature: None,
                    },
                    ContentBlock::ToolCall {
                        id: "call_2".into(),
                        name: "search".into(),
                        arguments: serde_json::json!({"query": "rust"}),
                        thought_signature: None,
                    },
                ],
                stop_reason: StopReason::ToolUse,
                ..partial_msg("")
            },
        },
    ];

    let stream = AssistantMessageEventStream::new(stream::iter(events));
    let msg = stream.result().await.unwrap();
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.content.len(), 2);
    match &msg.content[0] {
        ContentBlock::Text { text, .. } => assert_eq!(text, "Let me search for that. "),
        _ => panic!("expected Text as first block"),
    }
    match &msg.content[1] {
        ContentBlock::ToolCall { name, .. } => assert_eq!(name, "search"),
        _ => panic!("expected ToolCall as second block"),
    }
}
