//! Integration tests for pi-agent-core with DeepSeek V4 Flash.
//!
//! Tests multi-turn conversation and tool calling via the pi-ai streaming pipeline.
//! Requires `DEEPSEEK_API_KEY`.
//!
//! Run: cargo test --test deepseek_integration_test -- --ignored --nocapture

use std::sync::Arc;

use pi_agent_core::agent::{Agent, AgentOptions};
use pi_agent_core::pi_ai_types::{
    AssistantMessageEvent, ContentBlock, Context, Message, Model, ModelCost, StopReason,
    ThinkingLevel, Usage,
};
use pi_agent_core::types::{
    AgentMessage, AgentState, AgentTool, AgentToolResult, ConvertToLlmFn, StreamFn,
};
use pi_ai::providers::register_builtins::register_built_in_api_providers;

// ============================================================================
// Helpers
// ============================================================================

fn require_api_key() -> String {
    std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY must be set")
}

fn make_model() -> Model {
    Model {
        id: "deepseek-v4-flash".to_string(),
        name: "DeepSeek V4 Flash".to_string(),
        api: "openai-completions".to_string(),
        provider: "deepseek".to_string(),
        base_url: "https://api.deepseek.com".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".to_string()],
        cost: ModelCost::default(),
        context_window: 128000,
        max_tokens: 4096,
        headers: None,
        compat: None,
    }
}

fn make_tool_model() -> Model {
    let mut m = make_model();
    m.reasoning = false;
    m
}

fn make_stream_fn(api_key: &str) -> StreamFn {
    let key = api_key.to_string();
    Arc::new(
        move |model: Model,
              context: Context,
              _thinking: Option<ThinkingLevel>,
              _opts: pi_agent_core::types::StreamFnOptions|
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
                            Message::Assistant {
                                content,
                                api,
                                provider,
                                model,
                                usage,
                                stop_reason,
                                error_message,
                                timestamp,
                                ..
                            } => pi_ai::types::Message::Assistant {
                                content: content
                                    .iter()
                                    .map(|cb| match cb {
                                        ContentBlock::Text { text, text_signature } => {
                                            pi_ai::types::ContentBlock::Text {
                                                text: text.clone(),
                                                text_signature: text_signature.clone(),
                                            }
                                        }
                                        ContentBlock::Thinking {
                                            thinking,
                                            thinking_signature,
                                            redacted,
                                        } => pi_ai::types::ContentBlock::Thinking {
                                            thinking: thinking.clone(),
                                            thinking_signature: thinking_signature.clone(),
                                            redacted: *redacted,
                                        },
                                        ContentBlock::ToolCall {
                                            id,
                                            name,
                                            arguments,
                                            thought_signature,
                                        } => pi_ai::types::ContentBlock::ToolCall {
                                            id: id.clone(),
                                            name: name.clone(),
                                            arguments: arguments.clone(),
                                            thought_signature: thought_signature.clone(),
                                        },
                                        _ => pi_ai::types::ContentBlock::Text {
                                            text: String::new(),
                                            text_signature: None,
                                        },
                                    })
                                    .collect(),
                                api: api.clone(),
                                provider: provider.clone(),
                                model: model.clone(),
                                response_model: None,
                                response_id: None,
                                diagnostics: None,
                                usage: pi_ai::types::Usage {
                                    input: usage.input,
                                    output: usage.output,
                                    cache_read: usage.cache_read,
                                    cache_write: usage.cache_write,
                                    total_tokens: usage.total_tokens,
                                    cost: pi_ai::types::UsageCost {
                                        input: usage.cost.input,
                                        output: usage.cost.output,
                                        cache_read: usage.cost.cache_read,
                                        cache_write: usage.cost.cache_write,
                                        total: usage.cost.total,
                                    },
                                },
                                stop_reason: match stop_reason {
                                    StopReason::Stop => pi_ai::types::StopReason::Stop,
                                    StopReason::Length => pi_ai::types::StopReason::Length,
                                    StopReason::ToolUse => pi_ai::types::StopReason::ToolUse,
                                    StopReason::Error => pi_ai::types::StopReason::Error,
                                    StopReason::Aborted => pi_ai::types::StopReason::Aborted,
                                },
                                error_message: error_message.clone(),
                                timestamp: *timestamp,
                            },
                            Message::ToolResult {
                                tool_call_id,
                                tool_name,
                                content,
                                details,
                                is_error,
                                timestamp,
                            } => pi_ai::types::Message::ToolResult {
                                tool_call_id: tool_call_id.clone(),
                                tool_name: tool_name.clone(),
                                content: content.clone(),
                                details: details.clone(),
                                is_error: *is_error,
                                timestamp: *timestamp,
                            },
                            _ => pi_ai::types::Message::User {
                                content: vec![],
                                timestamp: 0,
                            },
                        })
                        .collect(),
                    tools: context.tools.as_ref().map(|tools| {
                        tools
                            .iter()
                            .map(|t| pi_ai::types::Tool {
                                name: t.name.clone(),
                                description: t.description.clone(),
                                parameters: t.parameters.clone(),
                            })
                            .collect()
                    }),
                };

                let stream_opts = pi_ai::types::StreamOptions {
                    api_key: Some(key),
                    ..Default::default()
                };

                let event_stream =
                    pi_ai::stream::stream(&pi_model, &pi_context, Some(stream_opts));

                use futures::StreamExt;
                let converted: pi_agent_core::pi_ai_types::StreamResponse = Box::new(
                    event_stream.map(|event| {
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
                                        pi_ai::types::ContentBlock::Image {
                                            data,
                                            mime_type,
                                        } => ContentBlock::Image {
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
                                diagnostics: None,
                                usage: Usage {
                                    input: msg.usage.input,
                                    output: msg.usage.output,
                                    cache_read: msg.usage.cache_read,
                                    cache_write: msg.usage.cache_write,
                                    total_tokens: msg.usage.total_tokens,
                                    cost: pi_agent_core::pi_ai_types::UsageCost {
                                        input: msg.usage.cost.input,
                                        output: msg.usage.cost.output,
                                        cache_read: msg.usage.cost.cache_read,
                                        cache_write: msg.usage.cost.cache_write,
                                        total: msg.usage.cost.total,
                                    },
                                },
                                stop_reason: match msg.stop_reason {
                                    pi_ai::types::StopReason::Stop => StopReason::Stop,
                                    pi_ai::types::StopReason::Length => StopReason::Length,
                                    pi_ai::types::StopReason::ToolUse => StopReason::ToolUse,
                                    pi_ai::types::StopReason::Error => StopReason::Error,
                                    pi_ai::types::StopReason::Aborted => StopReason::Aborted,
                                },
                                error_message: msg.error_message,
                                timestamp: msg.timestamp,
                            }
                        }
                        match event {
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
                            pi_ai::types::AssistantMessageEvent::Done { reason, message } => {
                                AssistantMessageEvent::Done {
                                    reason: match reason {
                                        pi_ai::types::StopReason::Stop => StopReason::Stop,
                                        pi_ai::types::StopReason::ToolUse => {
                                            StopReason::ToolUse
                                        }
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
                        }
                    }),
                );
                Ok(converted)
            })
        },
    )
}

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
// Test helpers
// ============================================================================

#[track_caller]
async fn timed_process(agent: &Agent, prompt: &str, label: &str) -> Vec<AgentMessage> {
    let start = std::time::Instant::now();
    let result = agent
        .process(vec![AgentMessage::User {
            content: vec![ContentBlock::Text {
                text: prompt.into(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }])
        .await;
    let elapsed = start.elapsed();
    println!("  [{label}] completed in {elapsed:.2?}");
    match result {
        Ok(msgs) => {
            let n_assistant = msgs.iter().filter(|m| matches!(m, AgentMessage::Assistant { .. })).count();
            let n_tool = msgs.iter().filter(|m| matches!(m, AgentMessage::ToolResult { .. })).count();
            println!("  [{label}] response: {} assistant + {} tool_result msgs", n_assistant, n_tool);
            for m in &msgs {
                if let AgentMessage::Assistant { content, stop_reason, .. } = m {
                    // Print all content blocks for debugging
                    for (i, cb) in content.iter().enumerate() {
                        match cb {
                            ContentBlock::Text { text, .. } => {
                                let preview: String = text.chars().take(80).collect();
                                println!("  [{label}] content[{i}]: Text({preview})");
                            }
                            ContentBlock::ToolCall { id, name, arguments, .. } => {
                                println!("  [{label}] content[{i}]: ToolCall(id={id}, name={name}, args={arguments})");
                            }
                            ContentBlock::Thinking { thinking, .. } => {
                                println!("  [{label}] content[{i}]: Thinking({} chars)", thinking.len());
                            }
                            _ => println!("  [{label}] content[{i}]: {:?}", cb),
                        }
                    }
                    let text: String = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    if !text.is_empty() {
                        println!("  [{label}] text: {text}");
                    }
                    if let Some(sr) = stop_reason {
                        println!("  [{label}] stop_reason: {sr:?}");
                    }
                }
            }
            msgs
        }
        Err(e) => panic!("{label} failed: {e}"),
    }
}

// ============================================================================
// Multi-turn conversation test (no tools)
// ============================================================================

#[tokio::test]
#[ignore = "requires DEEPSEEK_API_KEY and network"]
async fn test_deepseek_multi_turn_conversation() {
    register_built_in_api_providers();
    let api_key = require_api_key();
    let model = make_model();

    let total_start = std::time::Instant::now();
    println!("=== Multi-turn conversation test ===");

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "Answer questions concisely in one sentence. Do NOT use tool calls.".into(),
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
        stream_fn: Some(make_stream_fn(&api_key)),
        ..Default::default()
    });

    // Turn 1
    let r1 = timed_process(&agent, "What is 1+1?", "Turn 1").await;
    assert!(r1.iter().any(|m| matches!(m, AgentMessage::Assistant { .. })));

    // Turn 2: reference previous context
    let r2 = timed_process(&agent, "Multiply that result by 3.", "Turn 2").await;
    assert!(r2.iter().any(|m| matches!(m, AgentMessage::Assistant { .. })));
    let text: String = r2.iter()
        .filter_map(|m| match m {
            AgentMessage::Assistant { content, .. } => Some(
                content.iter().filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join(""),
            ),
            _ => None,
        })
        .collect::<Vec<_>>().join(" ");
    assert!(
        text.to_lowercase().contains('6') || text.to_lowercase().contains("six"),
        "Expected response to mention 6: {text}"
    );

    // Turn 3
    let r3 = timed_process(&agent, "What is the capital of France?", "Turn 3").await;
    let text3: String = r3.iter()
        .filter_map(|m| match m {
            AgentMessage::Assistant { content, .. } => Some(
                content.iter().filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join(""),
            ),
            _ => None,
        })
        .collect::<Vec<_>>().join(" ");
    assert!(
        text3.to_lowercase().contains("paris"),
        "Expected response to mention Paris: {text3}"
    );

    let state = agent.state().await;
    println!(
        "  Total test time: {:.2?}, messages in state: {}",
        total_start.elapsed(),
        state.messages.len()
    );
    assert_eq!(state.messages.len(), 6, "Expected 3 user + 3 assistant messages");
}

// ============================================================================
// Tool calling test — single turn
// ============================================================================

#[tokio::test]
#[ignore = "requires DEEPSEEK_API_KEY and network"]
async fn test_deepseek_tool_call() {
    register_built_in_api_providers();
    let api_key = require_api_key();
    let model = make_tool_model();

    let total_start = std::time::Instant::now();
    println!("=== Tool call test ===");

    let tool_call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = tool_call_count.clone();

    let add_tool = Arc::new(AgentTool {
        name: "add".into(),
        description: "Add two numbers and return the sum.".into(),
        label: "Add".into(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "First number" },
                "b": { "type": "number", "description": "Second number" }
            },
            "required": ["a", "b"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(
            move |id: String, args: serde_json::Value, _signal, _on_update| {
                let count = count.clone();
                Box::pin(async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    println!("  🔧 Tool 'add' called: id={id:?}, raw_args={args}");
                    let a = args["a"].as_f64().unwrap_or(0.0);
                    let b = args["b"].as_f64().unwrap_or(0.0);
                    let sum = a + b;
                    println!("  🔧 Tool 'add' result: {a} + {b} = {sum}");
                    Ok(AgentToolResult {
                        content: vec![ContentBlock::Text {
                            text: format!("{sum}"),
                            text_signature: None,
                        }],
                        details: serde_json::json!({"a": a, "b": b, "sum": sum}),
                        terminate: None,
                    })
                })
            },
        ),
    });

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant with an 'add' tool. Follow these rules exactly:\n1. When the user asks for a computation, call the add tool ONCE with the numbers.\n2. After the tool returns the result, respond to the user in a natural sentence. Do NOT call the tool again.\n3. NEVER make up a result — always use the tool.".into(),
            model,
            thinking_level: "off".into(),
            tools: vec![add_tool],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        convert_to_llm: Some(make_convert_to_llm()),
        stream_fn: Some(make_stream_fn(&api_key)),
        max_consecutive_tool_calls: Some(5),
        ..Default::default()
    });

    let result = timed_process(&agent, "Use the add tool to compute 15 + 25.", "ToolCall").await;

    let calls = tool_call_count.load(std::sync::atomic::Ordering::SeqCst);
    println!("  Tool was called {calls} time(s)");
    assert!(
        calls >= 1,
        "Expected at least one tool call, got {calls}"
    );
    assert!(
        calls <= 5,
        "Expected at most 5 tool calls (loop guard), got {calls}"
    );

    // Should end with assistant text response
    let has_final_assistant = result.iter().any(|m| {
        matches!(m, AgentMessage::Assistant { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::Text { .. })))
    });
    assert!(has_final_assistant, "Expected final assistant with text response");

    println!("  Total test time: {:.2?}", total_start.elapsed());
}

// ============================================================================
// Structured extraction test — schema-as-tool (Rig Extractor pattern)
// ============================================================================

#[derive(
    pi_agent_core::extraction::JsonSchema,
    serde::Deserialize,
    Debug,
    PartialEq,
)]
#[allow(dead_code)]
struct ExtractedPerson {
    name: String,
    age: u8,
    city: String,
}

#[tokio::test]
#[ignore = "requires DEEPSEEK_API_KEY and network"]
async fn test_deepseek_structured_extraction() {
    register_built_in_api_providers();
    let api_key = require_api_key();

    let model = pi_ai::types::Model {
        id: "deepseek-v4-flash".to_string(),
        name: "DeepSeek V4 Flash".to_string(),
        api: "openai-completions".to_string(),
        provider: "deepseek".to_string(),
        base_url: "https://api.deepseek.com".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".to_string()],
        cost: pi_ai::types::ModelCost::default(),
        context_window: 128000,
        max_tokens: 1024,
        headers: None,
        compat: None,
    };

    println!("=== Structured extraction test (schema-as-tool) ===");

    use pi_agent_core::extraction::Extractor;

    let extractor: Extractor<ExtractedPerson> = Extractor::new(model)
        .with_api_key(api_key)
        .with_tool_name("extract_person")
        .with_system_prompt(
            "Extract the person's information as structured data. \
             Call the extract_person tool with the correct fields.",
        );

    let start = std::time::Instant::now();
    let result = extractor
        .extract("John is 30 years old and lives in New York City.")
        .await;
    let elapsed = start.elapsed();

    match result {
        Ok(person) => {
            println!(
                "  Extracted ({elapsed:.2?}): {:?}",
                person
            );
            assert_eq!(person.name.to_lowercase(), "john");
            assert_eq!(person.age, 30);
            assert!(
                person.city.to_lowercase().contains("new york")
                    || person.city.to_lowercase().contains("nyc")
            );
            println!("  ✅ Structured extraction succeeded: name={}, age={}, city={}",
                person.name, person.age, person.city);
        }
        Err(e) => panic!("Extraction test failed: {e}"),
    }
}

// ============================================================================
// Tool calling test — multi-turn
// ============================================================================

#[tokio::test]
#[ignore = "requires DEEPSEEK_API_KEY and network"]
async fn test_deepseek_tool_call_multi_turn() {
    register_built_in_api_providers();
    let api_key = require_api_key();
    let model = make_tool_model();

    let total_start = std::time::Instant::now();
    println!("=== Tool call multi-turn test ===");

    let tool_call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = tool_call_count.clone();

    let adder_tool = Arc::new(AgentTool {
        name: "add".into(),
        description: "Add two numbers and return the sum.".into(),
        label: "Add".into(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "First number" },
                "b": { "type": "number", "description": "Second number" }
            },
            "required": ["a", "b"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(
            move |id: String, args: serde_json::Value, _signal, _on_update| {
                let count = count.clone();
                Box::pin(async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    println!("  🔧 Tool 'add' called: id={id:?}, raw_args={args}");
                    let a = args["a"].as_f64().unwrap_or(0.0);
                    let b = args["b"].as_f64().unwrap_or(0.0);
                    let sum = a + b;
                    println!("  🔧 Tool 'add' result: {a} + {b} = {sum}");
                    Ok(AgentToolResult {
                        content: vec![ContentBlock::Text {
                            text: format!("{sum}"),
                            text_signature: None,
                        }],
                        details: serde_json::json!({"a": a, "b": b, "sum": sum}),
                        terminate: None,
                    })
                })
            },
        ),
    });

    let agent = Agent::new(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant with an 'add' tool. Follow these rules exactly:\n1. When the user asks for a computation, call the add tool ONCE with the numbers.\n2. After the tool returns the result, respond to the user in a natural sentence. Do NOT call the tool again.\n3. NEVER make up a result — always use the tool.\n4. If the user asks to add to a previous result, call the tool again with the new numbers.".into(),
            model,
            thinking_level: "off".into(),
            tools: vec![adder_tool],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        convert_to_llm: Some(make_convert_to_llm()),
        stream_fn: Some(make_stream_fn(&api_key)),
        max_consecutive_tool_calls: Some(5),
        ..Default::default()
    });

    // Turn 1
    timed_process(&agent, "Use the add tool to compute 10 + 20.", "Turn 1").await;
    let n1 = tool_call_count.load(std::sync::atomic::Ordering::SeqCst);
    assert!(n1 >= 1, "Expected at least 1 tool call by end of turn 1, got {n1}");

    // Turn 2: reference previous result
    timed_process(&agent, "Now add 5 to the result from before.", "Turn 2").await;
    let n2 = tool_call_count.load(std::sync::atomic::Ordering::SeqCst);
    assert!(n2 >= 2, "Expected at least 2 tool calls total after turn 2, got {n2}");

    let state = agent.state().await;
    println!(
      "  Total test time: {:.2?}, messages in state: {}",
      total_start.elapsed(),
      state.messages.len()
    );
    assert!(
        state.messages.len() >= 6,
        "Expected at least 6 messages after 2 turns, got {}",
        state.messages.len()
    );
}
