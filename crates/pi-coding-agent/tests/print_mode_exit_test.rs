#![allow(clippy::unwrap_used, clippy::expect_used, clippy::type_complexity)]

use std::sync::Arc;

use pi_agent_core::pi_ai_types::{
    assistant_message, text_block, tool_call_block, AssistantMessageEvent, ContentBlock, Context,
    Message, Model, ModelCost, StopReason, StreamResponse, ThinkingLevel, Usage,
};
use pi_agent_core::types::StreamFn;
use pi_coding_agent::core::sdk::{create_agent_session, CreateAgentSessionOptions};
use pi_coding_agent::modes::print_mode::{run_print_mode, PrintModeOptions};

fn model() -> Model {
    Model {
        input_limits: None,
        prompt_cache: None,
        id: "mock-model".to_string(),
        name: "Mock Model".to_string(),
        api: "openai-completions".to_string(),
        provider: "mock".to_string(),
        base_url: "https://example.invalid/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".to_string()],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 4_096,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

/// First turn asks the real bash tool to run a command that exits 1. The next
/// turn receives the error tool result and completes normally.
fn tool_failure_then_stop_stream() -> StreamFn {
    Arc::new(
        move |model: Model,
              context: Context,
              _thinking_level: Option<ThinkingLevel>,
              _options: pi_agent_core::types::StreamFnOptions|
              -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<StreamResponse, Box<dyn std::error::Error + Send + Sync>>,
                    > + Send,
            >,
        > {
            Box::pin(async move {
                let response = match context.messages.last() {
                    Some(Message::ToolResult { tool_name, .. }) => {
                        assert_eq!(tool_name, "bash");
                        assistant_message(
                            vec![text_block("recovered after tool failure")],
                            model.api,
                            model.provider,
                            model.id,
                            Usage::default(),
                            StopReason::Stop,
                            chrono::Utc::now().timestamp_millis(),
                        )
                    }
                    _ => assistant_message(
                        vec![tool_call_block(
                            "call-false".to_string(),
                            "bash".to_string(),
                            serde_json::json!({"command": "false"}),
                        )],
                        model.api,
                        model.provider,
                        model.id,
                        Usage::default(),
                        StopReason::ToolUse,
                        chrono::Utc::now().timestamp_millis(),
                    ),
                };
                let stop = response.stop_reason.clone();
                let stream: StreamResponse =
                    Box::new(futures::stream::iter(vec![AssistantMessageEvent::Done {
                        reason: stop,
                        message: response,
                    }]));
                Ok(stream)
            })
        },
    )
}

/// A recoverable tool error must not turn a successful JSON-mode child into a
/// nonzero process exit. Subagents use this exit code to classify results.
#[tokio::test]
async fn json_mode_tool_failure_followed_by_stop_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (session, _result) = create_agent_session(CreateAgentSessionOptions {
        cwd: tmp.path().to_string_lossy().to_string(),
        agent_dir: Some(tmp.path().to_string_lossy().to_string()),
        model: Some(model()),
        thinking_level: Some("off".to_string()),
        scoped_models: None,
        no_tools: None,
        tools: None,
        exclude_tools: None,
        custom_prompt: None,
        append_system_prompt: None,
        session_name: None,
        stream_fn: Some(tool_failure_then_stop_stream()),
        convert_to_llm: None,
        custom_tools: None,
        tools_options: None,
        extension_flags: None,
        extension_paths: Vec::new(),
        enable_extensions: false,
        extension_registry: None,
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
        ui_context: None,
        trust_override: None,
    })
    .await
    .expect("create_agent_session failed");

    let exit_code = run_print_mode(PrintModeOptions {
        mode: "json",
        messages: &[],
        message: "run a command that fails",
        images: None::<&[ContentBlock]>,
        session,
        verbose: false,
    })
    .await;

    assert_eq!(exit_code, 0);
}
