//! Integration tests for OpenRouter connectivity via pi-ai.
//!
//! These tests require `OPENROUTER_API_KEY` to be set and use a free model.
//! Run with: `cargo test --test openrouter_integration_test -- --ignored --nocapture`

use pi_ai::models::get_model;
use pi_ai::providers::register_builtins::register_built_in_api_providers;
use pi_ai::types::{ContentBlock, Context, Message, ModelCost, StopReason, StreamOptions};
use std::env;

/// Get the test model ID from env or use a default free model.
fn test_model_id() -> String {
    env::var("PI_TEST_MODEL").unwrap_or_else(|_| {
        "poolside/laguna-xs.2:free".to_string()
    })
}

/// Build a Model for OpenRouter testing.
fn make_openrouter_model(model_id: &str) -> pi_ai::types::Model {
    pi_ai::types::Model {
        id: model_id.to_string(),
        name: format!("OpenRouter Test: {}", model_id),
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

fn require_api_key() -> String {
    env::var("OPENROUTER_API_KEY")
        .expect("OPENROUTER_API_KEY must be set for OpenRouter integration tests")
}

// ============================================================================
// pi-ai stream tests
// ============================================================================

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_pi_ai_stream_simple_text() {
    register_built_in_api_providers();
    let _api_key = require_api_key();
    let model_id = test_model_id();
    let model = make_openrouter_model(&model_id);

    let context = Context {
        system_prompt: Some("Reply with ONLY the word 'OK' and nothing else.".to_string()),
        messages: vec![Message::User {
            content: vec![ContentBlock::Text {
                text: "Say hello".to_string(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }],
        tools: None,
    };

    let stream = pi_ai::stream::stream(&model, &context, None);
    let result = stream.result().await;

    match result {
        Ok(msg) => {
            println!("stop_reason: {:?}", msg.stop_reason);
            println!("model: {} / {}", msg.provider, msg.model);
            println!("usage: input={}, output={}", msg.usage.input, msg.usage.output);
            let text: String = msg
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            println!("response: '{}'", text);
            println!("error_message: {:?}", msg.error_message);
            println!("stop_reason: {:?}", msg.stop_reason);
            if text.is_empty() && msg.error_message.is_some() {
                panic!("Stream error: {}", msg.error_message.as_ref().unwrap());
            }
            assert!(!text.is_empty(), "Response should contain text ({} content blocks)", msg.content.len());
            assert!(msg.usage.input > 0, "Should have input token usage");
        }
        Err(e) => panic!("Stream failed: {}", e),
    }
}

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_pi_ai_stream_with_temperature() {
    register_built_in_api_providers();
    let _api_key = require_api_key();
    let model_id = test_model_id();
    let model = make_openrouter_model(&model_id);

    let context = Context {
        system_prompt: Some("You are a helpful assistant. Answer briefly.".to_string()),
        messages: vec![Message::User {
            content: vec![ContentBlock::Text {
                text: "What is 1+1?".to_string(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }],
        tools: None,
    };

    let options = StreamOptions {
        temperature: Some(0.0),
        max_tokens: Some(100),
        ..Default::default()
    };

    let stream = pi_ai::stream::stream(&model, &context, Some(options));
    let result = stream.result().await;

    match result {
        Ok(msg) => {
            let text: String = msg
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            println!("response: '{}'", text);
            println!("error_message: {:?}", msg.error_message);
            println!("stop_reason: {:?}", msg.stop_reason);
            println!("content blocks: {}", msg.content.len());
            if text.is_empty() && msg.error_message.is_some() {
                panic!("Stream error: {}", msg.error_message.as_ref().unwrap());
            }
            assert!(text.contains('2'), "Response should contain '2', got: '{}'", text);
            assert!(
                msg.stop_reason == StopReason::Stop || msg.stop_reason == StopReason::Length,
                "Stop reason should be Stop or Length, got {:?}", msg.stop_reason
            );
        }
        Err(e) => panic!("Stream failed: {}", e),
    }
}

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_pi_ai_complete_simple() {
    register_built_in_api_providers();
    let _api_key = require_api_key();
    let model_id = test_model_id();
    let model = make_openrouter_model(&model_id);

    let context = Context {
        system_prompt: Some("Reply with EXACTLY 'pong' and nothing else.".to_string()),
        messages: vec![Message::User {
            content: vec![ContentBlock::Text {
                text: "ping".to_string(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }],
        tools: None,
    };

    // Use complete() which internally calls stream().result()
    let result = pi_ai::stream::complete(&model, &context, None).await;

    match result {
        Ok(msg) => {
            let text: String = msg
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            println!("response: {}", text);
            assert!(!text.is_empty());
            println!("Complete OK — {} chars, {} input tokens", text.len(), msg.usage.input);
        }
        Err(e) => panic!("Complete failed: {}", e),
    }
}

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_pi_ai_stream_with_system_prompt() {
    register_built_in_api_providers();
    let _api_key = require_api_key();
    let model_id = test_model_id();
    let model = make_openrouter_model(&model_id);

    let context = Context {
        system_prompt: Some(
            "You are a JSON-only API. Always respond with valid JSON: {\"answer\": \"...\"}."
                .to_string(),
        ),
        messages: vec![Message::User {
            content: vec![ContentBlock::Text {
                text: "What is the capital of France?".to_string(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }],
        tools: None,
    };

    let stream = pi_ai::stream::stream(&model, &context, None);
    let result = stream.result().await;

    match result {
        Ok(msg) => {
            let text: String = msg
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            println!("response: {}", text);
            assert!(text.contains("Paris"), "Response should mention Paris");
        }
        Err(e) => panic!("Stream with system prompt failed: {}", e),
    }
}
