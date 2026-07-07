pub mod handler;
pub mod jsonl;
pub mod rpc_types;

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::core::agent_session::AgentSession;
use crate::core::model_registry::ModelRegistry;
use crate::core::sdk::{create_agent_session, CreateAgentSessionOptions};

use super::rpc::handler::handle_command;
use super::rpc::jsonl::serialize_json_line;
use super::rpc::rpc_types::*;

/// Run the RPC mode: read JSON commands from stdin, output JSON events/responses
/// to stdout, and drive the agent session.
///
/// Protocol:
/// - Commands: JSON objects with `type` field on stdin (one per line)
/// - Responses: JSON objects on stdout with `type: "response"`
/// - Events: JSON objects on stdout with `type: "event"` streamed as they occur
pub async fn run_rpc_mode() -> i32 {
    // ── Build a minimal agent session ──────────────────────────────────
    let agent_dir = crate::config::get_agent_dir();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/tmp".to_string());

    let sdk_options = CreateAgentSessionOptions {
        cwd: cwd.clone(),
        agent_dir: Some(agent_dir.to_string_lossy().to_string()),
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
        extension_paths: Vec::new(),
        enable_extensions: false, persist_session: false, session_file: None,
        cli_provider: None,
        cli_model: None,
    };

    let (mut session, _result) = match create_agent_session(sdk_options).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("RPC init error: {e}");
            return 1;
        }
    };

    let model_registry = ModelRegistry::new(ModelRegistry::builtin_models_list());

    // ── Event streaming channel ────────────────────────────────────────
    let (mut handler_state, mut event_rx) = handler::RpcHandlerState::new();

    // Spawn a task to flush event channel to stdout
    tokio::spawn(async move {
        while let Some(line) = event_rx.recv().await {
            use std::io::Write;
            let mut handle = std::io::stdout().lock();
            let _ = handle.write_all(line.as_bytes());
            let _ = handle.flush();
        }
    });

    // ── Main loop: read JSON commands from stdin ──────────────────────
    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();
    let mut shutdown = false;

    while !shutdown {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break, // EOF
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        // Parse command
        let command: RpcCommand = match serde_json::from_str(&line) {
            Ok(cmd) => cmd,
            Err(e) => {
                let err = rpc_error(None, "parse", format!("Invalid JSON: {e}"));
                let out = serialize_json_line(&err);
                print!("{out}");
                use std::io::Write;
                std::io::stdout().flush().ok();
                continue;
            }
        };

        // Handle command
        let response = handle_command(command, &mut session, &model_registry, &mut handler_state).await;

        // Write synchronous response (async responses go through event channel)
        if let Some(output) = response {
            let out = serialize_json_line(&output);
            print!("{out}");
            use std::io::Write;
            std::io::stdout().flush().ok();
        }

        if handler_state.shutdown_requested {
            shutdown = true;
        }
    }

    0
}
