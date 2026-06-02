//! Integration tests for real LLM connectivity.
//!
//! Tests connectivity to OpenRouter and DeepSeek via the pi-ai streaming pipeline.
//! API keys are loaded from environment variables via `env_api_keys.rs`:
//!   - OpenRouter  → `OPENROUTER_API_KEY`
//!   - DeepSeek    → `DEEPSEEK_API_KEY`
//!
//! Run: cargo test --test connection_test -- --ignored --nocapture

use pi_ai::models::get_model;
use pi_ai::providers::register_builtins::register_built_in_api_providers;
use pi_ai::stream::stream;
use pi_ai::types::{ContentBlock, Context, Message, StopReason, StreamOptions};

/// Helper: one-shot streaming call, returns the final AssistantMessage.
async fn stream_one(model_id: &str, provider: &str, prompt: &str) -> pi_ai::types::AssistantMessage {
    let _ = register_built_in_api_providers();

    let model = get_model(provider, model_id).unwrap_or_else(|| {
        panic!("Model {}/{} not found in generated catalog. Run build.rs first.", provider, model_id)
    });

    let context = Context {
        system_prompt: Some(format!(r#"You are a terse assistant. Reply in one short sentence. Do NOT use tool calls. Reply directly with plain text.

User: {prompt}

Assistant:"#)),
        messages: vec![Message::User {
            content: vec![ContentBlock::text("Hi")],
            timestamp: 0,
        }],
        tools: None,
    };

    let event_stream = stream(&model, &context, Some(StreamOptions {
        max_tokens: Some(512),
        ..Default::default()
    }));
    event_stream.result().await.unwrap_or_else(|e| {
        panic!("Stream failed for {}/{}: {}", provider, model_id, e)
    })
}

// ============================================================================
// OpenRouter /free
// ============================================================================

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_openrouter_free_model_found_in_catalog() {
    register_built_in_api_providers();
    let model = get_model("openrouter", "openrouter/free")
        .expect("openrouter/free not found in generated catalog");
    assert_eq!(model.provider, "openrouter");
    assert_eq!(model.api, "openai-completions");
    assert_eq!(model.base_url, "https://openrouter.ai/api/v1");
}

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and network"]
async fn test_openrouter_free_can_stream() {
    let result = stream_one("openrouter/free", "openrouter", "Say exactly: pong").await;
    println!("openrouter/free response: {:?}", result);
    assert_eq!(result.stop_reason, StopReason::Stop,
        "Expected StopReason::Stop, got {:?}. Error: {:?}",
        result.stop_reason, result.error_message);
}

// ============================================================================
// DeepSeek V4 Flash
// ============================================================================

#[tokio::test]
#[ignore = "requires DEEPSEEK_API_KEY and network"]
async fn test_deepseek_v4_flash_model_found_in_catalog() {
    register_built_in_api_providers();
    let model = get_model("deepseek", "deepseek-v4-flash")
        .expect("deepseek-v4-flash not found in generated catalog");
    assert_eq!(model.provider, "deepseek");
    assert_eq!(model.api, "openai-completions");
    assert_eq!(model.base_url, "https://api.deepseek.com");
}

#[tokio::test]
#[ignore = "requires DEEPSEEK_API_KEY and network"]
async fn test_deepseek_v4_flash_can_stream() {
    let result = stream_one("deepseek-v4-flash", "deepseek", "Say exactly: pong").await;
    println!("deepseek-v4-flash response: {:?}", result);
    assert_eq!(result.stop_reason, StopReason::Stop,
        "Expected StopReason::Stop, got {:?}. Error: {:?}",
        result.stop_reason, result.error_message);
}
