use std::env;
use std::sync::Arc;

use pi_agent_core::agent::PromptInput;
use pi_agent_core::pi_ai_types::{
    AssistantMessageEvent, ContentBlock, Context, Message, Model, ModelCost, StopReason,
    ThinkingLevel,
};
use pi_agent_core::types::{AgentMessage, StreamFn};

use pi_coding_agent::core::sdk::{create_agent_session, CreateAgentSessionOptions};

use pi_extension_api::ExtensionRegistry;

/// Create an `ExtensionRegistry` with the goal extension (exercises the
/// extension-tool path alongside the built-in tools).
pub fn create_registry() -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    registry.register(Box::new(pi_extensions::goal::GoalExtension::new()));
    registry
}

// ============================================================================
// OpenRouter LLM bridge (requires OPENROUTER_API_KEY + network)
// ============================================================================

fn require_api_key() -> String {
    env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY must be set")
}

fn test_model_id() -> String {
    env::var("PI_TEST_MODEL").unwrap_or_else(|_| "poolside/laguna-m.1:free".to_string())
}

fn make_model() -> Model {
    let id = test_model_id();
    Model {
        id: id.clone(),
        name: format!("Test: {id}"),
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

/// Build a real `StreamFn` that forwards to OpenRouter, preserving the tool
/// definitions so the LLM can see (and report) the available tools.
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
                let pi_model = pi_agent_core::pi_ai::types::Model {
                    id: model.id,
                    name: model.name,
                    api: model.api,
                    provider: model.provider,
                    base_url: model.base_url,
                    reasoning: model.reasoning,
                    thinking_level_map: model.thinking_level_map,
                    input: model.input,
                    cost: pi_agent_core::pi_ai::types::ModelCost {
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

                let pi_context = pi_agent_core::pi_ai::types::Context {
                    system_prompt: context.system_prompt,
                    messages: context
                        .messages
                        .iter()
                        .map(|m| match m {
                            Message::User { content, timestamp } => {
                                pi_agent_core::pi_ai::types::Message::User {
                                    content: content.clone(),
                                    timestamp: *timestamp,
                                }
                            }
                            Message::Assistant {
                                content,
                                api,
                                provider,
                                model,
                                timestamp,
                                ..
                            } => pi_agent_core::pi_ai::types::Message::Assistant {
                                content: content.clone(),
                                api: api.clone(),
                                provider: provider.clone(),
                                model: model.clone(),
                                response_model: None,
                                response_id: None,
                                diagnostics: None,
                                usage: pi_agent_core::pi_ai::types::Usage::default(),
                                stop_reason: pi_agent_core::pi_ai::types::StopReason::Stop,
                                error_message: None,
                                timestamp: *timestamp,
                            },
                            Message::ToolResult {
                                tool_call_id,
                                tool_name,
                                content,
                                details,
                                is_error,
                                timestamp,
                            } => pi_agent_core::pi_ai::types::Message::ToolResult {
                                tool_call_id: tool_call_id.clone(),
                                tool_name: tool_name.clone(),
                                content: content.clone(),
                                details: details.clone(),
                                is_error: *is_error,
                                timestamp: *timestamp,
                            },
                        })
                        .collect(),
                    // Forward the tool definitions so the LLM can see them.
                    tools: context.tools.map(|tools| {
                        tools
                            .iter()
                            .map(|t| pi_agent_core::pi_ai::types::Tool {
                                name: t.name.clone(),
                                description: t.description.clone(),
                                parameters: t.parameters.clone(),
                            })
                            .collect()
                    }),
                };

                let stream_opts = pi_agent_core::pi_ai::types::StreamOptions {
                    api_key: Some(key),
                    ..Default::default()
                };

                let event_stream =
                    pi_agent_core::pi_ai::stream::stream(&pi_model, &pi_context, Some(stream_opts));

                use futures::StreamExt;
                let converted: pi_agent_core::pi_ai_types::StreamResponse =
                    Box::new(event_stream.map(|event| match event {
                        pi_agent_core::pi_ai::types::AssistantMessageEvent::Start { partial } => {
                            AssistantMessageEvent::Start {
                                partial: convert_msg(partial),
                            }
                        }
                        pi_agent_core::pi_ai::types::AssistantMessageEvent::TextStart {
                            content_index,
                            partial,
                        } => AssistantMessageEvent::TextStart {
                            content_index,
                            partial: convert_msg(partial),
                        },
                        pi_agent_core::pi_ai::types::AssistantMessageEvent::TextDelta {
                            content_index,
                            delta,
                            partial,
                        } => AssistantMessageEvent::TextDelta {
                            content_index,
                            delta,
                            partial: convert_msg(partial),
                        },
                        pi_agent_core::pi_ai::types::AssistantMessageEvent::TextEnd {
                            content_index,
                            content,
                            partial,
                        } => AssistantMessageEvent::TextEnd {
                            content_index,
                            content,
                            partial: convert_msg(partial),
                        },
                        pi_agent_core::pi_ai::types::AssistantMessageEvent::Done {
                            reason,
                            message,
                        } => AssistantMessageEvent::Done {
                            reason: match reason {
                                pi_agent_core::pi_ai::types::StopReason::Stop => StopReason::Stop,
                                pi_agent_core::pi_ai::types::StopReason::ToolUse => {
                                    StopReason::ToolUse
                                }
                                pi_agent_core::pi_ai::types::StopReason::Aborted => {
                                    StopReason::Aborted
                                }
                                _ => StopReason::Error,
                            },
                            message: convert_msg(message),
                        },
                        pi_agent_core::pi_ai::types::AssistantMessageEvent::Error {
                            reason: _,
                            error,
                        } => AssistantMessageEvent::Error {
                            reason: StopReason::Error,
                            error: convert_msg(error),
                        },
                        other => match other {
                            pi_agent_core::pi_ai::types::AssistantMessageEvent::ThinkingStart {
                                content_index,
                                partial,
                            } => AssistantMessageEvent::ThinkingStart {
                                content_index,
                                partial: convert_msg(partial),
                            },
                            pi_agent_core::pi_ai::types::AssistantMessageEvent::ThinkingDelta {
                                content_index,
                                delta,
                                partial,
                            } => AssistantMessageEvent::ThinkingDelta {
                                content_index,
                                delta,
                                partial: convert_msg(partial),
                            },
                            pi_agent_core::pi_ai::types::AssistantMessageEvent::ThinkingEnd {
                                content_index,
                                content,
                                partial,
                            } => AssistantMessageEvent::ThinkingEnd {
                                content_index,
                                content,
                                partial: convert_msg(partial),
                            },
                            pi_agent_core::pi_ai::types::AssistantMessageEvent::ToolCallStart {
                                content_index,
                                partial,
                            } => AssistantMessageEvent::ToolCallStart {
                                content_index,
                                partial: convert_msg(partial),
                            },
                            pi_agent_core::pi_ai::types::AssistantMessageEvent::ToolCallDelta {
                                content_index,
                                delta,
                                partial,
                            } => AssistantMessageEvent::ToolCallDelta {
                                content_index,
                                delta,
                                partial: convert_msg(partial),
                            },
                            pi_agent_core::pi_ai::types::AssistantMessageEvent::ToolCallEnd {
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
                                error: convert_msg(pi_agent_core::pi_ai::types::AssistantMessage {
                                    content: vec![],
                                    api: String::new(),
                                    provider: String::new(),
                                    model: String::new(),
                                    response_model: None,
                                    response_id: None,
                                    diagnostics: None,
                                    usage: pi_agent_core::pi_ai::types::Usage::default(),
                                    stop_reason: pi_agent_core::pi_ai::types::StopReason::Error,
                                    error_message: Some("Unknown event".into()),
                                    timestamp: 0,
                                }),
                            },
                        },
                    }));

                Ok(converted)
            })
        },
    )
}

fn convert_msg(
    msg: pi_agent_core::pi_ai::types::AssistantMessage,
) -> pi_agent_core::pi_ai_types::AssistantMessage {
    pi_agent_core::pi_ai_types::AssistantMessage {
        content: msg
            .content
            .iter()
            .map(|cb| match cb {
                pi_agent_core::pi_ai::types::ContentBlock::Text {
                    text,
                    text_signature,
                } => ContentBlock::Text {
                    text: text.clone(),
                    text_signature: text_signature.clone(),
                },
                pi_agent_core::pi_ai::types::ContentBlock::Thinking {
                    thinking,
                    thinking_signature,
                    redacted,
                } => ContentBlock::Thinking {
                    thinking: thinking.clone(),
                    thinking_signature: thinking_signature.clone(),
                    redacted: *redacted,
                },
                pi_agent_core::pi_ai::types::ContentBlock::ToolCall {
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
                pi_agent_core::pi_ai::types::ContentBlock::Image { data, mime_type } => {
                    ContentBlock::Image {
                        data: data.clone(),
                        mime_type: mime_type.clone(),
                    }
                }
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
// Tests
// ============================================================================

#[tokio::test]
async fn test_create_agent_session_with_extension() {
    let ext_registry = create_registry();
    let (session, _result) = create_agent_session(CreateAgentSessionOptions {
        cwd: ".".to_string(),
        agent_dir: None,
        model: None,
        thinking_level: None,
        scoped_models: None,
        no_tools: None,
        tools: None,
        exclude_tools: None,
        custom_prompt: None,
        append_system_prompt: None,
        session_name: None,
        stream_fn: None,
        convert_to_llm: None,
        custom_tools: None,
        extension_paths: Vec::new(),
        enable_extensions: true,
        extension_registry: Some(ext_registry),
        cli_provider: None,
        cli_model: None,
        persist_session: true,
        session_file: None,
        fork_from: None,
        session_dir: None,
        auth_storage: None,
        model_registry: None,
        resource_loader: None,
        session_manager: None,
        settings_manager: None,
        session_start_event: None,
    })
    .await
    .expect("create_agent_session failed");
    assert!(session.get_extension_registry().is_some());
    let active_tool_names = session.get_active_tool_names().await;
    println!("[dbg] active tools: {:#?}", active_tool_names);
    let all_tools = session.get_all_tools();
    println!(
        "[dbg] all tools: {:#?}",
        all_tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // Find and invoke the `create_goal` extension tool through the active tool list.
    let agent_state = session.get_agent().state().await;
    let create_goal = agent_state
        .tools
        .iter()
        .find(|t| t.name == "create_goal")
        .expect("create_goal tool should be active");
    let params =
        serde_json::json!({ "objective": "Write a test that exercises the goal extension" });
    let result = (create_goal.execute)("call-create-goal-1".to_string(), params, None, None)
        .await
        .expect("create_goal execute failed");
    let goal_text = result
        .content
        .iter()
        .filter_map(|c| {
            if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = c {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    println!("[dbg] create_goal result: {}", goal_text);
}

#[tokio::test]
async fn test_session_builtin_bash_tool_exec() {
    let ext_registry = create_registry();
    let (session, _result) = create_agent_session(CreateAgentSessionOptions {
        cwd: ".".to_string(),
        agent_dir: None,
        model: None,
        thinking_level: None,
        scoped_models: None,
        no_tools: None,
        tools: None,
        exclude_tools: None,
        custom_prompt: None,
        append_system_prompt: None,
        session_name: None,
        stream_fn: None,
        convert_to_llm: None,
        custom_tools: None,
        extension_paths: Vec::new(),
        enable_extensions: true,
        extension_registry: Some(ext_registry),
        cli_provider: None,
        cli_model: None,
        persist_session: true,
        session_file: None,
        fork_from: None,
        session_dir: None,
        auth_storage: None,
        model_registry: None,
        resource_loader: None,
        session_manager: None,
        settings_manager: None,
        session_start_event: None,
    })
    .await
    .expect("create_agent_session failed");
    assert!(session.get_extension_registry().is_some());
    let active_tool_names = session.get_active_tool_names().await;
    println!("[dbg] active tools: {:#?}", active_tool_names);
    let all_tools = session.get_all_tools();
    println!(
        "[dbg] all tools: {:#?}",
        all_tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // Find and invoke the `bash` built-in tool to run `ls`.
    let agent_state = session.get_agent().state().await;
    let bash = agent_state
        .tools
        .iter()
        .find(|t| t.name == "bash")
        .expect("bash tool should be active");
    let bash_params = serde_json::json!({ "command": "ls" });
    let bash_result = (bash.execute)("call-bash-1".to_string(), bash_params, None, None)
        .await
        .expect("bash execute failed");
    let bash_text = bash_result
        .content
        .iter()
        .filter_map(|c| {
            if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = c {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    println!("[dbg] bash `ls` result:\n{}", bash_text);
    assert!(bash_text.contains("Cargo.toml"));
    assert!(bash_text.contains("tests"));
}

/// Ask the LLM what tools it has access to and assert the reply mentions the
/// built-in `bash` tool. Requires `OPENROUTER_API_KEY` and network access.
#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_llm_detect_builtin_tools() {
    let api_key = require_api_key();
    let ext_registry = create_registry();
    let (session, _result) = create_agent_session(CreateAgentSessionOptions {
        cwd: ".".to_string(),
        agent_dir: None,
        model: Some(make_model()),
        thinking_level: Some("off".to_string()),
        scoped_models: None,
        no_tools: None,
        tools: None,
        exclude_tools: None,
        custom_prompt: None,
        append_system_prompt: None,
        session_name: None,
        stream_fn: Some(make_openrouter_stream_fn(&api_key)),
        convert_to_llm: None,
        custom_tools: None,
        extension_paths: Vec::new(),
        enable_extensions: true,
        extension_registry: Some(ext_registry),
        cli_provider: None,
        cli_model: None,
        persist_session: false,
        session_file: None,
        fork_from: None,
        session_dir: None,
        auth_storage: None,
        model_registry: None,
        resource_loader: None,
        session_manager: None,
        settings_manager: None,
        session_start_event: None,
    })
    .await
    .expect("create_agent_session failed");

    let active_tool_names = session.get_active_tool_names().await;
    println!("[dbg] active tools sent to LLM: {:#?}", active_tool_names);
    assert!(
        active_tool_names.contains(&"bash".to_string()),
        "bash must be in the active tool list before asking the LLM"
    );

    let messages = session
        .get_agent()
        .prompt(PromptInput::Text(
            "List the tools you currently have access to, one name per line.",
        ))
        .await
        .expect("agent prompt failed");

    let reply: String = messages
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

    println!("[dbg] LLM reply:\n{}", reply);
    assert!(
        reply.to_lowercase().contains("bash"),
        "Expected LLM to report the built-in bash tool, but reply was: {reply}"
    );
}

/// Ask the LLM what tools it has access to and assert the reply mentions the
/// built-in `bash` tool. Requires `OPENROUTER_API_KEY` and network access.
#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_invoke_agent() {
    let api_key = require_api_key();
    let ext_registry = create_registry();
    let (session, _result) = create_agent_session(CreateAgentSessionOptions {
        cwd: ".".to_string(),
        agent_dir: None,
        model: Some(make_model()),
        thinking_level: Some("off".to_string()),
        scoped_models: None,
        no_tools: None,
        tools: None,
        exclude_tools: None,
        custom_prompt: None,
        append_system_prompt: None,
        session_name: None,
        stream_fn: Some(make_openrouter_stream_fn(&api_key)),
        convert_to_llm: None,
        custom_tools: None,
        extension_paths: Vec::new(),
        enable_extensions: true,
        extension_registry: Some(ext_registry),
        cli_provider: None,
        cli_model: None,
        persist_session: false,
        session_file: None,
        fork_from: None,
        session_dir: None,
        auth_storage: None,
        model_registry: None,
        resource_loader: None,
        session_manager: None,
        settings_manager: None,
        session_start_event: None,
    })
    .await
    .expect("create_agent_session failed");

    let messages = session
        .get_agent()
        .prompt(PromptInput::Text("执行ls命令"))
        .await
        .expect("agent prompt failed");

    // Print all messages for debugging
    println!("[dbg] === All messages ({}) ===", messages.len());
    for (i, msg) in messages.iter().enumerate() {
        match msg {
            AgentMessage::Assistant { content, .. } => {
                println!("[dbg]   [{i}] Assistant ({} blocks):", content.len());
                for (j, block) in content.iter().enumerate() {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            println!("[dbg]     [{j}] Text: {text}");
                        }
                        ContentBlock::ToolCall { id, name, arguments, .. } => {
                            println!("[dbg]     [{j}] ToolCall: id={id} name={name} args={arguments}");
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            println!("[dbg]     [{j}] Thinking: {thinking}");
                        }
                        _ => {
                            println!("[dbg]     [{j}] Other: {block:?}");
                        }
                    }
                }
            }
            AgentMessage::ToolResult { tool_call_id, tool_name, content, is_error, .. } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                println!("[dbg]   [{i}] ToolResult: tool={tool_name} id={tool_call_id} is_error={is_error} text={text}");
            }
            AgentMessage::BashExecution { command, output, exit_code, .. } => {
                println!("[dbg]   [{i}] BashExecution: cmd={command} exit={exit_code:?} output={output}");
            }
            _ => {
                println!("[dbg]   [{i}] Other: {msg:?}");
            }
        }
    }

    // Verify that the LLM made at least one tool call (bash with ls)
    let has_tool_call = messages.iter().any(|m| match m {
        AgentMessage::Assistant { content, .. } => content.iter().any(|b| matches!(b, ContentBlock::ToolCall { .. })),
        _ => false,
    });
    assert!(has_tool_call, "Expected at least one tool call in assistant messages");

    // Verify that at least one tool result came back
    let has_tool_result = messages.iter().any(|m| matches!(m, AgentMessage::ToolResult { .. }));
    assert!(has_tool_result, "Expected at least one tool result");
}

/// Build a deterministic, offline `StreamFn` that drives the multi-turn tool
/// communication test without hitting a real LLM. The agent loop + real
/// built-in `bash` tool are exercised end-to-end; only the assistant
/// responses are canned. This keeps the test repeatable (per the project
/// guideline that LLM-dependent tests should use recorded fixtures / mocks
/// rather than live network calls).
// The boxed-future return type mirrors `StreamFn` and is inherently verbose;
#[allow(clippy::type_complexity)]
fn make_mock_multi_turn_stream_fn() -> StreamFn {
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
            Box::pin(async move {
                use pi_agent_core::pi_ai_types::{
                    assistant_message, text_block, tool_call_block, AssistantMessageEvent,
                    Message, StreamResponse, StopReason, Usage,
                };

                let api = model.api.clone();
                let provider = model.provider.clone();
                let model_id = model.id.clone();
                let now = chrono::Utc::now().timestamp_millis();

                // The agent loop calls stream_fn once per LLM turn. A turn that
                // ends with `StopReason::ToolUse` triggers tool execution and a
                // follow-up stream_fn call whose last context message is a
                // `ToolResult`. We branch on the last converted message so the
                // mock is order-independent and robust to extra steering
                // messages.
                let response = match context.messages.last() {
                    Some(Message::User { content, .. }) => {
                        let user_text: String = content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text, .. } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        if user_text.contains("创建") || user_text.contains("create") {
                            // Turn 1: create the file via `bash echo`.
                            assistant_message(
                                vec![tool_call_block(
                                    "call_echo_1".to_string(),
                                    "bash".to_string(),
                                    serde_json::json!({"command": "echo \"hello world\" > test.txt"}),
                                )],
                                api,
                                provider,
                                model_id,
                                Usage::default(),
                                StopReason::ToolUse,
                                now,
                            )
                        } else {
                            // Turn 2: read the file via `bash cat`.
                            assistant_message(
                                vec![tool_call_block(
                                    "call_cat_1".to_string(),
                                    "bash".to_string(),
                                    serde_json::json!({"command": "cat test.txt"}),
                                )],
                                api,
                                provider,
                                model_id,
                                Usage::default(),
                                StopReason::ToolUse,
                                now,
                            )
                        }
                    }
                    Some(Message::ToolResult { tool_name, .. }) => {
                        // After tool execution the assistant produces a final
                        // text response referencing the file content.
                        let text = if tool_name == "bash" {
                            "The file test.txt contains: hello world".to_string()
                        } else {
                            "Done.".to_string()
                        };
                        assistant_message(
                            vec![text_block(text)],
                            api,
                            provider,
                            model_id,
                            Usage::default(),
                            StopReason::Stop,
                            now,
                        )
                    }
                    _ => assistant_message(
                        vec![text_block("Done.")],
                        api,
                        provider,
                        model_id,
                        Usage::default(),
                        StopReason::Stop,
                        now,
                    ),
                };

                let stop = response.stop_reason.clone();
                let event = AssistantMessageEvent::Done {
                    reason: stop,
                    message: response,
                };
                let stream: StreamResponse = Box::new(futures::stream::iter(vec![event]));
                Ok(stream)
            })
        },
    )
}

/// Multi-turn conversation test: send two related prompts and verify the agent
/// maintains context across turns. Also exercises tool-to-tool communication:
/// turn 1 creates a file (bash echo), turn 2 reads it back (bash cat).
///
/// Uses a deterministic mock `StreamFn` (no network / API key required) so the
/// test is repeatable, while still exercising the real agent loop and the real
/// built-in `bash` tool.
#[tokio::test]
async fn test_multi_turn_tool_communication() {
    let ext_registry = create_registry();

    // Use a temp directory so file operations don't pollute the workspace.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cwd = tmp.path().to_string_lossy().to_string();

    let (session, _result) = create_agent_session(CreateAgentSessionOptions {
        cwd: cwd.clone(),
        agent_dir: None,
        model: Some(make_model()),
        thinking_level: Some("off".to_string()),
        scoped_models: None,
        no_tools: None,
        tools: None,
        exclude_tools: None,
        custom_prompt: None,
        append_system_prompt: None,
        session_name: None,
        stream_fn: Some(make_mock_multi_turn_stream_fn()),
        convert_to_llm: None,
        custom_tools: None,
        extension_paths: Vec::new(),
        enable_extensions: true,
        extension_registry: Some(ext_registry),
        cli_provider: None,
        cli_model: None,
        persist_session: false,
        session_file: None,
        fork_from: None,
        session_dir: None,
        auth_storage: None,
        model_registry: None,
        resource_loader: None,
        session_manager: None,
        settings_manager: None,
        session_start_event: None,
    })
    .await
    .expect("create_agent_session failed");

    // ── Turn 1: create a file ──────────────────────────────────────────
    println!("[test] === Turn 1: create file ===");
    let turn1_messages = session
        .get_agent()
        .prompt(PromptInput::Text(
            &format!(
                "在目录 {} 下创建一个名为 test.txt 的文件，内容为 hello world，使用 bash 的 echo 命令",
                cwd
            ),
        ))
        .await
        .expect("turn 1 prompt failed");

    // Print turn 1 messages
    println!("[test] Turn 1 messages ({}):", turn1_messages.len());
    for (i, msg) in turn1_messages.iter().enumerate() {
        match msg {
            AgentMessage::Assistant { content, .. } => {
                for block in content.iter() {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            println!("[test]   [{i}] Assistant Text: {text}");
                        }
                        ContentBlock::ToolCall { id, name, arguments, .. } => {
                            println!("[test]   [{i}] ToolCall: id={id} name={name} args={arguments}");
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            println!("[test]   [{i}] Thinking: {thinking}");
                        }
                        _ => {}
                    }
                }
            }
            AgentMessage::ToolResult { tool_call_id, tool_name, content, is_error, .. } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                println!("[test]   [{i}] ToolResult: tool={tool_name} id={tool_call_id} is_error={is_error} text={text}");
            }
            AgentMessage::BashExecution { command, output, exit_code, .. } => {
                println!("[test]   [{i}] BashExecution: cmd={command} exit={exit_code:?} output={output}");
            }
            _ => {
                println!("[test]   [{i}] Other: {msg:?}");
            }
        }
    }

    // Verify turn 1 had a tool call (bash with echo)
    let turn1_tool_calls: Vec<&AgentMessage> = turn1_messages
        .iter()
        .filter(|m| {
            matches!(m, AgentMessage::Assistant { content, .. } if content.iter().any(|b| matches!(b, ContentBlock::ToolCall { name, .. } if name == "bash")))
        })
        .collect();
    assert!(
        !turn1_tool_calls.is_empty(),
        "Turn 1 should have a bash tool call to create the file"
    );

    // Verify turn 1 had a tool result (bash output)
    let turn1_tool_results: Vec<&AgentMessage> = turn1_messages
        .iter()
        .filter(|m| matches!(m, AgentMessage::ToolResult { .. }))
        .collect();
    assert!(
        !turn1_tool_results.is_empty(),
        "Turn 1 should have at least one tool result"
    );

    // Verify the file was actually created
    let file_path = std::path::Path::new(&cwd).join("test.txt");
    assert!(
        file_path.exists(),
        "File test.txt should exist after turn 1"
    );
    let content = std::fs::read_to_string(&file_path).expect("read test.txt");
    println!("[test] File content after turn 1: {content:?}");
    assert!(
        content.trim() == "hello world",
        "File should contain 'hello world', got {content:?}"
    );

    // ── Turn 2: read the file back ─────────────────────────────────────
    println!("[test] === Turn 2: read file ===");
    // Show the messages already in agent state before turn 2.
    let pre_messages = session.get_agent().messages().await;
    println!("[test] Pre-turn-2 agent state messages ({})", pre_messages.len());
    for (i, msg) in pre_messages.iter().enumerate() {
        match msg {
            AgentMessage::User { content, .. } => {
                let texts: Vec<&str> = content.iter().filter_map(|b| match b { ContentBlock::Text { text, .. } => Some(text.as_str()), _ => None }).collect();
                println!("[test]   pre[{i}] User: {:?}", texts);
            }
            AgentMessage::Assistant { content, stop_reason, .. } => {
                let texts: Vec<&str> = content.iter().filter_map(|b| match b { ContentBlock::Text { text, .. } => Some(text.as_str()), _ => None }).collect();
                println!("[test]   pre[{i}] Assistant: {:?} stop={:?}", texts, stop_reason);
            }
            AgentMessage::ToolResult { tool_name, is_error, .. } => {
                println!("[test]   pre[{i}] ToolResult: tool={} is_error={}", tool_name, is_error);
            }
            _ => {
                println!("[test]   pre[{i}] Other: {:?}", std::mem::discriminant(msg));
            }
        }
    }
    let turn2_messages = session
        .get_agent()
        .prompt(PromptInput::Text("读取 test.txt 的内容，使用 bash 的 cat 命令"))
        .await
        .expect("turn 2 prompt failed");

    // Print turn 2 messages
    println!("[test] Turn 2 messages ({}):", turn2_messages.len());
    for (i, msg) in turn2_messages.iter().enumerate() {
        match msg {
            AgentMessage::Assistant { content, .. } => {
                for block in content.iter() {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            println!("[test]   [{i}] Assistant Text: {text}");
                        }
                        ContentBlock::ToolCall { id, name, arguments, .. } => {
                            println!("[test]   [{i}] ToolCall: id={id} name={name} args={arguments}");
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            println!("[test]   [{i}] Thinking: {thinking}");
                        }
                        _ => {}
                    }
                }
            }
            AgentMessage::ToolResult { tool_call_id, tool_name, content, is_error, .. } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                println!("[test]   [{i}] ToolResult: tool={tool_name} id={tool_call_id} is_error={is_error} text={text}");
            }
            AgentMessage::BashExecution { command, output, exit_code, .. } => {
                println!("[test]   [{i}] BashExecution: cmd={command} exit={exit_code:?} output={output}");
            }
            _ => {
                println!("[test]   [{i}] Other: {msg:?}");
            }
        }
    }

    // Verify turn 2 had a tool call (bash with cat)
    let turn2_tool_calls: Vec<&AgentMessage> = turn2_messages
        .iter()
        .filter(|m| {
            matches!(m, AgentMessage::Assistant { content, .. } if content.iter().any(|b| matches!(b, ContentBlock::ToolCall { name, .. } if name == "bash")))
        })
        .collect();
    assert!(
        !turn2_tool_calls.is_empty(),
        "Turn 2 should have a bash tool call to read the file"
    );

    // Verify turn 2 had a tool result
    let turn2_tool_results: Vec<&AgentMessage> = turn2_messages
        .iter()
        .filter(|m| matches!(m, AgentMessage::ToolResult { .. }))
        .collect();
    assert!(
        !turn2_tool_results.is_empty(),
        "Turn 2 should have at least one tool result"
    );

    // Verify the final assistant response mentions "hello world" or the file content
    let final_text: String = turn2_messages
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

    println!("[test] Final assistant text: {final_text}");
    assert!(
        final_text.to_lowercase().contains("hello world")
            || final_text.to_lowercase().contains("hello")
            || final_text.to_lowercase().contains("test.txt"),
        "Final response should mention the file content, got: {final_text}"
    );

    // ── Verify agent state has all messages ────────────────────────────
    let all_messages = session.get_agent().messages().await;
    println!("[test] Total messages in agent state: {}", all_messages.len());
    assert!(
        all_messages.len() >= 4,
        "Agent state should have at least 4 messages (2 user + 2 assistant), got {}",
        all_messages.len()
    );
}
