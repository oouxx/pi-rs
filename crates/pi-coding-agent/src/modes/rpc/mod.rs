pub mod handler;
pub mod jsonl;
pub mod rpc_types;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::core::agent_session::AgentSessionEvent;
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
        enable_extensions: true,
        persist_session: false,
        session_file: None,
        fork_from: None,
        session_dir: None,
        extension_registry: None,
        cli_provider: None,
        cli_model: None,
        auth_storage: None,
        model_registry: None,
        resource_loader: None,
        session_manager: None,
        settings_manager: None,
        session_start_event: None,
        custom_tools: None,
    };

    let (mut session, _result) = match create_agent_session(sdk_options).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("RPC init error: {e}");
            return 1;
        }
    };

    let model_registry = ModelRegistry::new(ModelRegistry::builtin_models_list());

    // ── Single output channel for all stdout writes ────────────────────
    // Both event streaming and synchronous responses go through this channel,
    // ensuring no interleaving on stdout.
    let (output_tx, mut output_rx) = mpsc::unbounded_channel::<String>();

    // Spawn a single writer task that owns stdout
    tokio::spawn(async move {
        use std::io::Write;
        while let Some(line) = output_rx.recv().await {
            let mut handle = std::io::stdout().lock();
            let _ = handle.write_all(line.as_bytes());
            let _ = handle.flush();
        }
    });

    // ── Signal handling ────────────────────────────────────────────────
    // Track which signal was received for correct exit code (matching TS:
    // SIGHUP → 129, SIGTERM → 143). Use an atomic to communicate to main loop.
    let signal_received = Arc::new(AtomicU8::new(0));
    let sig_recv = signal_received.clone();

    let mut term_signal = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::terminate(),
    )
    .ok();
    let mut hang_signal = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::hangup(),
    )
    .ok();

    tokio::spawn(async move {
        tokio::select! {
            _ = async {
                if let Some(ref mut sig) = term_signal { sig.recv().await; }
                sig_recv.store(1, Ordering::SeqCst); // SIGTERM
            } => {}
            _ = async {
                if let Some(ref mut sig) = hang_signal { sig.recv().await; }
                sig_recv.store(2, Ordering::SeqCst); // SIGHUP
            } => {}
        }
    });

    // ── Event streaming setup ──────────────────────────────────────────
    let mut handler_state = handler::RpcHandlerState::new(output_tx.clone());



    // ── Global session event subscription ──────────────────────────────
    // Subscribe to ALL session events and forward them to stdout,
    // matching TS: session.subscribe((event) => { output(event); ... })
    let output_tx_for_session = output_tx.clone();
    let _session_event_handle = session.subscribe_session_events(
        std::sync::Arc::new(move |event: AgentSessionEvent| {
            // Serialize event to JSON and forward to output channel
            if let Ok(json) = serde_json::to_value(&event) {
                let line = serialize_json_line(&json);
                let _ = output_tx_for_session.send(line);
            }
            // Check for agent_settled to trigger shutdown check (matching TS)
            if matches!(event, AgentSessionEvent::AgentSettled) {
                // If shutdown was requested, the main loop will handle it
                // via handler_state.shutdown_requested
            }
        }),
    );

    // ── Main loop: read JSON commands from stdin ──────────────────────
    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();
    let mut shutdown = false;

    // Shutdown re-entrancy guard (matching TS `shuttingDown` flag)
    while !shutdown {
        // Check signal flag (matching TS signal handlers with correct exit codes)
        let sig = signal_received.load(Ordering::SeqCst);
        if sig != 0 {
            // Kill tracked detached children (matching TS killTrackedDetachedChildren())
            crate::utils::shell::kill_tracked_detached_children();
            session.dispose_inner().await;
            // Match TS exit codes: SIGHUP → 129, SIGTERM → 143
            return if sig == 2 { 129 } else { 143 };
        }

        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break, // EOF
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        // Parse as generic Value first to extract id and type for error reporting,
        // and to check for extension_ui_response (matching TS behavior).
        let parsed_value: Option<serde_json::Value> = serde_json::from_str(&line).ok();
        let cmd_type = parsed_value.as_ref().and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|s| s.to_string()));
        let cmd_id: Option<String> = parsed_value.as_ref().and_then(|v| v.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()));

        // Check for extension_ui_response before parsing as RpcCommand
        // (matching TS: checks parsed.type === "extension_ui_response" first)
        if cmd_type.as_deref() == Some("extension_ui_response") {
            if let Some(ref response_id) = cmd_id {
                if let Some(val) = parsed_value {
                    let pending = handler_state.pending_extension_requests.lock().unwrap().remove(response_id);
                    if let Some(sender) = pending {
                        let _ = sender.send(val);
                    }
                }
            }
            continue;
        }

        // Parse command (matching TS: command = parsed as RpcCommand)
        let command: RpcCommand = match serde_json::from_str(&line) {
            Ok(cmd) => cmd,
            Err(e) => {
                // Match TS: error(id, command.type, "Unknown command: ...") for unknown types,
                // or error(undefined, "parse", "Failed to parse command: ...") for invalid JSON
                let err = if let Some(ref ct) = cmd_type {
                    rpc_error(cmd_id, ct, format!("Unknown command: {ct}"))
                } else {
                    rpc_error(None, "parse", format!("Failed to parse command: {e}"))
                };
                let out = serialize_json_line(&err);
                let _ = output_tx.send(out);
                continue;
            }
        };

        // Handle command
        let response = handle_command(
            command,
            &mut session,
            &model_registry,
            &mut handler_state,
        )
        .await;

        // Write synchronous response through the shared output channel
        if let Some(output) = response {
            let out = serialize_json_line(&output);
            let _ = output_tx.send(out);
        }

        if handler_state.shutdown_requested {
            shutdown = true;
        }
    }

    session.dispose_inner().await;
    0
}
