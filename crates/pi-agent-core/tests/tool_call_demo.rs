//! Real tool call demo — reads OPENROUTER_API_KEY from .env and makes one tool call.
//!
//! Run: cargo test --test tool_call_demo -- --nocapture

use std::sync::Arc;

use pi_agent_core::agent::{Agent, AgentOptions};
use pi_agent_core::pi_ai_types::{
    ContentBlock, Model, ModelCost, StopReason, ThinkingLevel, Usage,
};
use pi_agent_core::types::{AgentMessage, AgentTool, AgentToolResult, ConvertToLlmFn, StreamFn};
use pi_ai::providers::register_builtins::register_built_in_api_providers;

fn load_env() {
    let content = std::fs::read_to_string("../../.env").unwrap_or_default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            std::env::set_var(key.trim(), val.trim());
        }
    }
}

fn make_stream_fn() -> StreamFn {
    Arc::new(
        move |model: Model,
              context: pi_agent_core::pi_ai_types::Context,
              _thinking: Option<ThinkingLevel>,
              _opts: pi_agent_core::types::StreamFnOptions|
              -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<pi_agent_core::pi_ai_types::StreamResponse, Box<dyn std::error::Error + Send + Sync>>> + Send>,
        > {
            let key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
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
                    messages: context.messages.iter().map(|m| match m {
                        pi_agent_core::pi_ai_types::Message::User { content, timestamp } => {
                            pi_ai::types::Message::User {
                                content: content.iter().map(|cb| match cb {
                                    ContentBlock::Text { text, .. } => pi_ai::types::ContentBlock::Text {
                                        text: text.clone(),
                                        text_signature: None,
                                    },
                                    _ => pi_ai::types::ContentBlock::Text { text: String::new(), text_signature: None },
                                }).collect(),
                                timestamp: *timestamp,
                            }
                        }
                        _ => pi_ai::types::Message::User { content: vec![], timestamp: 0 },
                    }).collect(),
                    tools: context.tools.as_ref().map(|tools| {
                        tools.iter().map(|t| pi_ai::types::Tool {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            parameters: t.parameters.clone(),
                        }).collect()
                    }),
                };

                let stream_opts = pi_ai::types::StreamOptions {
                    api_key: Some(key),
                    ..Default::default()
                };

                let event_stream = pi_ai::stream::stream(&pi_model, &pi_context, Some(stream_opts));

                use futures::StreamExt;
                let converted: pi_agent_core::pi_ai_types::StreamResponse = Box::new(
                    event_stream.map(|event| {
                        fn convert_msg(msg: pi_ai::types::AssistantMessage) -> pi_agent_core::pi_ai_types::AssistantMessage {
                            pi_agent_core::pi_ai_types::AssistantMessage {
                                content: msg.content.iter().map(|cb| match cb {
                                    pi_ai::types::ContentBlock::Text { text, text_signature } => ContentBlock::Text {
                                        text: text.clone(), text_signature: text_signature.clone(),
                                    },
                                    pi_ai::types::ContentBlock::Thinking { thinking, thinking_signature, redacted } => ContentBlock::Thinking {
                                        thinking: thinking.clone(), thinking_signature: thinking_signature.clone(), redacted: *redacted,
                                    },
                                    pi_ai::types::ContentBlock::ToolCall { id, name, arguments, thought_signature } => ContentBlock::ToolCall {
                                        id: id.clone(), name: name.clone(), arguments: arguments.clone(), thought_signature: thought_signature.clone(),
                                    },
                                    pi_ai::types::ContentBlock::Image { data, mime_type } => ContentBlock::Image {
                                        data: data.clone(), mime_type: mime_type.clone(),
                                    },
                                }).collect(),
                                api: msg.api,
                                provider: msg.provider,
                                model: msg.model,
                                response_model: msg.response_model,
                                response_id: msg.response_id,
                                diagnostics: msg.diagnostics.map(|diags| {
                                    diags.into_iter().map(|d| pi_agent_core::pi_ai_types::AssistantMessageDiagnostic {
                                        content_index: d.content_index,
                                        diagnostic: d.diagnostic,
                                        severity: match d.severity {
                                            pi_ai::types::DiagnosticSeverity::Warning => pi_ai::types::DiagnosticSeverity::Warning,
                                            pi_ai::types::DiagnosticSeverity::Error => pi_ai::types::DiagnosticSeverity::Error,
                                        },
                                    }).collect()
                                }),
                                usage: Usage {
                                    input: msg.usage.input, output: msg.usage.output,
                                    cache_read: msg.usage.cache_read, cache_write: msg.usage.cache_write,
                                    total_tokens: msg.usage.total_tokens,
                                    cost: pi_agent_core::pi_ai_types::UsageCost {
                                        input: msg.usage.cost.input, output: msg.usage.cost.output,
                                        cache_read: msg.usage.cost.cache_read, cache_write: msg.usage.cost.cache_write,
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
                            pi_ai::types::AssistantMessageEvent::Start { partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::Start { partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::TextStart { content_index, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::TextStart { content_index, partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::TextDelta { content_index, delta, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::TextDelta { content_index, delta, partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::TextEnd { content_index, content, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::TextEnd { content_index, content, partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::ThinkingStart { content_index, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::ThinkingStart { content_index, partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::ThinkingDelta { content_index, delta, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::ThinkingDelta { content_index, delta, partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::ThinkingEnd { content_index, content, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::ThinkingEnd { content_index, content, partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::ToolCallStart { content_index, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::ToolCallStart { content_index, partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::ToolCallDelta { content_index, delta, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::ToolCallDelta { content_index, delta, partial: convert_msg(partial) },
                            pi_ai::types::AssistantMessageEvent::ToolCallEnd { content_index, tool_call, partial } => pi_agent_core::pi_ai_types::AssistantMessageEvent::ToolCallEnd {
                                content_index,
                                tool_call: pi_agent_core::pi_ai_types::ToolCall {
                                    type_field: "toolCall".to_string(), id: tool_call.id, name: tool_call.name,
                                    arguments: tool_call.arguments, thought_signature: None,
                                },
                                partial: convert_msg(partial),
                            },
                            pi_ai::types::AssistantMessageEvent::Done { reason, message } => pi_agent_core::pi_ai_types::AssistantMessageEvent::Done {
                                reason: match reason { pi_ai::types::StopReason::Stop => StopReason::Stop, pi_ai::types::StopReason::ToolUse => StopReason::ToolUse, _ => StopReason::Error },
                                message: convert_msg(message),
                            },
                            pi_ai::types::AssistantMessageEvent::Error { reason: _, error } => pi_agent_core::pi_ai_types::AssistantMessageEvent::Error {
                                reason: StopReason::Error, error: convert_msg(error),
                            },
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
        messages.iter().filter_map(|m| match m {
            AgentMessage::User { content, timestamp } => {
                Some(pi_agent_core::pi_ai_types::Message::User { content: content.clone(), timestamp: *timestamp })
            }
            AgentMessage::Assistant { content, api, provider, model, usage, stop_reason, error_message, timestamp } => {
                Some(pi_agent_core::pi_ai_types::Message::Assistant {
                    content: content.clone(), api: api.clone(), provider: provider.clone(),
                    model: model.clone(), response_model: None, response_id: None, diagnostics: None,
                    usage: usage.clone(), stop_reason: stop_reason.clone().unwrap_or(StopReason::Stop),
                    error_message: error_message.clone(), timestamp: *timestamp,
                })
            }
            AgentMessage::ToolResult { tool_call_id, tool_name, content, details, is_error, timestamp } => {
                Some(pi_agent_core::pi_ai_types::Message::ToolResult {
                    tool_call_id: tool_call_id.clone(), tool_name: tool_name.clone(),
                    content: content.clone(), details: Some(details.clone()),
                    is_error: *is_error, timestamp: *timestamp,
                })
            }
            _ => None,
        }).collect()
    })
}

#[tokio::test]
async fn test_tool_call_demo() {
    load_env();
    register_built_in_api_providers();

    let _api_key = std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY not found in .env");
    let model_id = "deepseek-chat"; // known tool-capable model

    let model = Model {
        id: model_id.to_string(),
        name: format!("Test: {}", model_id),
        api: "openai-completions".to_string(),
        provider: "deepseek".to_string(),
        base_url: "https://api.deepseek.com".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".to_string()],
        cost: ModelCost::default(),
        context_window: 128000,
        max_tokens: 4096,
        headers: None,
        compat: None,
    };

    println!("\n--- Tool Call Demo (DeepSeek) ---");
    println!("Model: {}", model_id.to_string());

    // ── Define a simple adder tool ──
    let adder_tool = Arc::new(AgentTool {
        name: "add".into(),
        description: "Add two numbers and return the sum".into(),
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
        execute: Arc::new(|_id: String, args: serde_json::Value, _signal, _on_update| {
            Box::pin(async move {
                let a = args["a"].as_f64().unwrap_or(0.0);
                let b = args["b"].as_f64().unwrap_or(0.0);
                let sum = a + b;
                println!("  🔧 Tool 'add' called: {} + {} = {}", a, b, sum);
                Ok(AgentToolResult {
                    content: vec![ContentBlock::Text {
                        text: format!("The sum of {} and {} is {}.", a, b, sum),
                        text_signature: None,
                    }],
                    details: serde_json::json!({"a": a, "b": b, "sum": sum}),
                    terminate: Some(true),
                })
            })
        }),
    });

    let agent = Agent::new(AgentOptions {
        initial_state: Some(pi_agent_core::types::AgentState {
            system_prompt:             "You are a helpful assistant with access to an 'add' tool. When the user asks you to compute something, you MUST use the add tool — do NOT compute the answer yourself. Use the tool even for simple arithmetic.".into(),
            model: model.clone(),
            thinking_level: "off".into(),
            tools: vec![adder_tool],
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        }),
        convert_to_llm: Some(make_convert_to_llm()),
        stream_fn: Some(make_stream_fn()),
        ..Default::default()
    });

    println!("  Sending prompt: 'Use the add tool to compute 12345 + 67890'");
    println!();

    let result = agent
        .process(vec![AgentMessage::User {
            content: vec![ContentBlock::Text {
                text: "Use the add tool to compute 12345 + 67890".into(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }])
        .await;

    match result {
        Ok(messages) => {
            println!("\n  ✅ Agent completed with {} new messages", messages.len());
            for msg in &messages {
                match msg {
                    AgentMessage::Assistant { content, stop_reason, usage, .. } => {
                        let text: String = content.iter().filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        }).collect::<Vec<_>>().join(" ");
                        println!("  🤖 Assistant (stop={:?}, tokens={}): {}",
                            stop_reason, usage.total_tokens, text);
                    }
                    AgentMessage::ToolResult { tool_name, content, is_error, .. } => {
                        let text: String = content.iter().filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        }).collect::<Vec<_>>().join(" ");
                        println!("  🔧 ToolResult [{}] {}: {}", if *is_error { "ERROR" } else { "OK" }, tool_name, text);
                    }
                    AgentMessage::User { content, .. } => {
                        let text: String = content.iter().filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        }).collect::<Vec<_>>().join(" ");
                        println!("  💬 User: {}", text);
                    }
                    _ => {}
                }
            }
        }
        Err(e) => {
            println!("  ❌ Agent error: {}", e);
        }
    }

    println!();
}
