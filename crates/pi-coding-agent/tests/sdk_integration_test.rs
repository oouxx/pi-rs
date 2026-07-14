//! SDK integration test — tests AgentSession with a real LLM provider.
//!
//! Run with:
//!   DEEPSEEK_API_KEY=sk-... cargo test -p pi-coding-agent --test sdk_integration_test -- --nocapture --include-ignored

use std::sync::Arc;

use pi_agent_core::pi_ai_types::AssistantMessageEvent;
use pi_agent_core::types::AgentEvent;

/// Test the stream_fn bridge directly.
#[tokio::test]
#[ignore = "Requires DEEPSEEK_API_KEY"]
async fn test_stream_fn_bridge() {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("Set DEEPSEEK_API_KEY");
    pi_ai::providers::register_builtins::register_built_in_api_providers();

    let models = pi_ai::models::get_models("deepseek");
    let pi_model = models.iter().find(|m| m.id == "deepseek-chat")
        .or_else(|| models.first()).cloned().expect("No models");

    // Create the SDK context format
    let ctx = pi_agent_core::pi_ai_types::Context {
        system_prompt: Some("You are helpful.".into()),
        messages: vec![pi_agent_core::pi_ai_types::Message::User {
            content: vec![pi_agent_core::pi_ai_types::ContentBlock::Text { text: "say hi".into(), text_signature: None }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }],
        tools: None,
    };

    // Use the SDK's default stream_fn
    let stream_fn = pi_coding_agent::core::sdk::create_default_stream_fn();

    // Both types are the same (pi_agent_core re-exports from pi_ai)
    let model: pi_agent_core::pi_ai_types::Model = pi_model;

    let opts = pi_agent_core::types::StreamFnOptions {
        api_key: Some(key),
        headers: None,
        signal: None,
        session_id: None,
        thinking_budgets: None,
        max_retry_delay_ms: None,
        transport: None,
        on_payload: None,
        on_response: None,
    };

    let result = stream_fn(model, ctx, None, opts).await;
    match result {
        Ok(mut stream) => {
            use futures::StreamExt;
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                match event {
                    AssistantMessageEvent::TextDelta { delta, .. } => text.push_str(&delta),
                    AssistantMessageEvent::Error { error, .. } => {
                        panic!("Stream error: {:?}", error);
                    }
                    AssistantMessageEvent::Done { .. } => break,
                    _ => {}
                }
            }
            eprintln!("[bridge] '{text}'");
            assert!(!text.is_empty(), "Empty response from bridge");
        }
        Err(e) => panic!("StreamFn failed: {e}"),
    }
}

/// Test that pi-ai's streaming works directly (baseline test).
#[tokio::test]
#[ignore = "Requires DEEPSEEK_API_KEY"]
async fn test_pi_ai_direct() {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("Set DEEPSEEK_API_KEY");
    pi_ai::providers::register_builtins::register_built_in_api_providers();

    let models = pi_ai::models::get_models("deepseek");
    let model = models.iter().find(|m| m.id == "deepseek-chat")
        .or_else(|| models.first()).cloned().expect("No DeepSeek models");

    let mut opts = pi_ai::types::SimpleStreamOptions::default();
    opts.base.api_key = Some(key);

    let ctx = pi_ai::types::Context {
        system_prompt: Some("You are helpful.".into()),
        messages: vec![pi_ai::types::Message::User {
            content: vec![pi_ai::types::ContentBlock::Text { text: "say hi".into(), text_signature: None }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }],
        tools: None,
    };

    let msg = pi_ai::stream::complete_simple(&model, &ctx, Some(opts))
        .await.expect("complete_simple failed");
    let text: String = msg.content.iter()
        .filter_map(|b| if let pi_ai::types::ContentBlock::Text { text, .. } = b { Some(text.clone()) } else { None })
        .collect();
    eprintln!("[direct] '{text}'");
    assert!(!text.is_empty(), "Expected non-empty response");
}

/// Test full SDK flow: create_agent_session → add_user_text → response.
#[tokio::test]
#[ignore = "Requires DEEPSEEK_API_KEY"]
async fn test_sdk_full_flow() {
    let _key = std::env::var("DEEPSEEK_API_KEY").expect("Set DEEPSEEK_API_KEY");
    pi_ai::providers::register_builtins::register_built_in_api_providers();

    let cwd = std::env::current_dir().unwrap().to_string_lossy().to_string();
    let agent_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target").join(".pi-rs-test-agent");

    let options = pi_coding_agent::core::sdk::CreateAgentSessionOptions {
        cwd, agent_dir: Some(agent_dir.to_string_lossy().to_string()),
        model: None, thinking_level: None, scoped_models: None,
        no_tools: None, tools: None, exclude_tools: None,
        custom_prompt: None, append_system_prompt: None,
        session_name: None, stream_fn: None, convert_to_llm: None,
        extension_paths: vec![], enable_extensions: false, persist_session: false, session_file: None,
        fork_from: None, session_dir: None, extension_registry: None,
        cli_provider: None, cli_model: None,
    };

    let (mut session, _result) = pi_coding_agent::core::sdk::create_agent_session(options)
        .await.expect("create_agent_session failed");

    let response_text = Arc::new(tokio::sync::Mutex::new(String::new()));
    let rt = response_text.clone();

    let listener: Arc<dyn Fn(AgentEvent, Option<tokio::sync::watch::Receiver<bool>>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync> =
        Arc::new(move |event, _signal| {
            let rt = rt.clone();
            Box::pin(async move {
                eprintln!("[evt] {:?}", std::mem::discriminant(&event));
                match &event {
                    AgentEvent::MessageUpdate { assistant_message_event, .. } => {
                        eprintln!("[evt] MessageUpdate: {assistant_message_event:?}");
                        if let AssistantMessageEvent::TextDelta { delta, .. } = assistant_message_event {
                            print!("{delta}");
                            std::io::Write::flush(&mut std::io::stdout()).ok();
                            rt.lock().await.push_str(delta);
                        }
                    }
                    AgentEvent::MessageEnd { message: msg } => {
                        eprintln!("[evt] MessageEnd");
                        if let pi_agent_core::types::AgentMessage::Assistant { content, .. } = msg {
                            let text: String = content.iter()
                                .filter_map(|b| if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = b { Some(text.clone()) } else { None })
                                .collect();
                            eprintln!("[evt] MessageEnd text: '{text}'");
                            if !text.is_empty() {
                                rt.lock().await.push_str(&text);
                            }
                        }
                    }
                    _ => {}
                }
            })
        });

    session.subscribe(listener).await;
    session.add_user_text("say hello in one word").await;
    session.wait_for_idle().await;

    let text = response_text.lock().await.clone();
    if text.is_empty() {
        let state = session.get_agent().state().await;
        eprintln!("[sdk] Agent error: {:?}", state.error_message);
        eprintln!("[sdk] Messages count: {}", state.messages.len());
        for (i, msg) in state.messages.iter().enumerate() {
            match msg {
                pi_agent_core::types::AgentMessage::Assistant { content, error_message, .. } => {
                    let t: String = content.iter().filter_map(|b| if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = b { Some(text.clone()) } else { None }).collect();
                    eprintln!("[sdk] Msg[{i}] Assistant: '{t}' err={error_message:?}");
                }
                pi_agent_core::types::AgentMessage::User { content, .. } => {
                    let t: String = content.iter().filter_map(|b| if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = b { Some(text.clone()) } else { None }).collect();
                    eprintln!("[sdk] Msg[{i}] User: '{t}'");
                }
                _ => eprintln!("[sdk] Msg[{i}] {:?}", msg),
            }
        }
    } else {
        eprintln!("[sdk] OK: '{text}'");
    }
    assert!(!text.is_empty(), "Expected non-empty response");
}
