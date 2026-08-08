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

/// Mirror of TS `modes/json-event.ts` `toJsonEvent()`.
///
/// The RPC wire format matches the TS `AgentSessionEvent` shape:
/// a `type`-discriminated union with camelCase fields. The one difference
/// from the in-memory event is that `message_update` events strip the
/// cumulative `partial` assistant snapshot from `assistantMessageEvent`
/// (`message_start` provides the initial message, deltas build it, and
/// `message_end` provides the final authoritative message).
fn to_json_event(event: &AgentSessionEvent) -> serde_json::Value {
    let mut value = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
    if let Some(ame) = value.get_mut("assistantMessageEvent") {
        if let Some(obj) = ame.as_object_mut() {
            obj.remove("partial");
        }
    }
    value
}

/// Run the RPC mode: read JSON commands from stdin, output JSON events/responses
/// to stdout, and drive the agent session.
///
/// Protocol:
/// - Commands: JSON objects with `type` field on stdin (one per line)
/// - Responses: JSON objects on stdout with `type: "response"`
/// - Events: JSON objects on stdout with `type: "event"` streamed as they occur
pub async fn run_rpc_mode(
    extension_paths: Vec<String>,
    extension_flags: std::collections::HashMap<String, String>,
) -> i32 {
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
        extension_paths,
        extension_flags: Some(extension_flags),
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

    let mut rpc_builtins = ModelRegistry::builtin_models_list();
    rpc_builtins.extend(pi_agent_core::pi_ai::providers::ollama::discover_ollama_models().await);
    let model_registry = ModelRegistry::new(rpc_builtins);

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
            // Serialize event to JSON and forward to output channel.
            // Use to_json_event() to match the TS RPC wire shape exactly
            // (type-discriminated union, camelCase fields, no `partial` snapshot).
            let json = to_json_event(&event);
            let line = serialize_json_line(&json);
            let _ = output_tx_for_session.send(line);
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
                    let pending = handler_state.pending_extension_requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(response_id);
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


#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use pi_agent_core::pi_ai_types::{AssistantMessage, AssistantMessageEvent, StopReason, Usage};

    fn sample_assistant_message() -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        }
    }

    /// The RPC wire format must match the TS `AgentSessionEvent` shape:
    /// a `type`-discriminated union with camelCase fields (TS
    /// `packages/coding-agent/src/core/agent-session.ts` + `modes/json-event.ts`).
    #[test]
    fn event_wire_format_matches_ts() {
        // unit variant -> {"type":"agent_start"}
        let e = AgentSessionEvent::AgentStart;
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"type":"agent_start"}"#);

        // message_update -> type tag + camelCase fields
        let am = sample_assistant_message();
        let e2 = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            })).unwrap(),
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
                partial: am,
            },
        };
        let v = serde_json::to_value(&e2).unwrap();
        assert_eq!(v["type"], "message_update");
        assert_eq!(v["assistantMessageEvent"]["type"], "text_delta");
        assert_eq!(v["assistantMessageEvent"]["contentIndex"], 0);
        assert_eq!(v["assistantMessageEvent"]["delta"], "hi");

        // tool_execution_start -> camelCase fields
        let e3 = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "a.txt"}),
        };
        let v3 = serde_json::to_value(&e3).unwrap();
        assert_eq!(v3["type"], "tool_execution_start");
        assert_eq!(v3["toolCallId"], "t1");
        assert_eq!(v3["toolName"], "read");
        assert_eq!(v3["args"]["path"], "a.txt");

        // compaction_end -> camelCase fields (willRetry/errorMessage)
        let e4 = AgentSessionEvent::CompactionEnd {
            reason: crate::core::agent_session::CompactionReason::Manual,
            result: None,
            aborted: false,
            will_retry: false,
            error_message: None,
        };
        let v4 = serde_json::to_value(&e4).unwrap();
        assert_eq!(v4["type"], "compaction_end");
        assert!(v4.get("willRetry").is_some(), "compaction_end must use willRetry");
        assert!(v4.get("will_retry").is_none());
        assert!(v4.get("errorMessage").is_some(), "compaction_end must use errorMessage");
    }

    /// `to_json_event` strips the cumulative `partial` snapshot from
    /// `message_update` events, mirroring TS `toJsonEvent()`.
    #[test]
    fn to_json_event_strips_partial() {
        let am = sample_assistant_message();
        let e = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            })).unwrap(),
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
                partial: am,
            },
        };
        let v = to_json_event(&e);
        assert_eq!(v["type"], "message_update");
        assert!(v["assistantMessageEvent"].get("partial").is_none(),
            "partial must be stripped on the wire (TS toJsonEvent)");
        assert_eq!(v["assistantMessageEvent"]["delta"], "hi");
    }
}
