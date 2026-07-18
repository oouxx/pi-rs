//! RPC command handler — dispatches RPC commands to AgentSession.
//!
//! Mirrors the command handling in packages/coding-agent/src/modes/rpc/rpc-mode.ts

use std::pin::Pin;
use std::sync::Arc;

use crate::core::agent_session::AgentSession;
use crate::core::model_registry::ModelRegistry;
use crate::core::session_manager::SessionEntry;

use super::jsonl::serialize_json_line;
use super::rpc_types::*;

/// Handle a single RPC command, returning an optional output message.
/// Returns `None` for commands that produce output asynchronously (e.g. `prompt`).
pub async fn handle_command(
    command: RpcCommand,
    session: &mut AgentSession,
    model_registry: &ModelRegistry,
    state: &mut RpcHandlerState,
) -> Option<RpcOutput> {
    match command {
        // ── Prompting ─────────────────────────────────────────────────────
        RpcCommand::Prompt {
            id,
            message,
            images: _,
            streaming_behavior: _,
        } => {
            let event_tx = state.event_tx.clone();
            let done_tx = state.done_tx.clone();

            let listener: Arc<
                dyn Fn(
                        pi_agent_core::types::AgentEvent,
                        Option<tokio::sync::watch::Receiver<bool>>,
                    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                    + Send
                    + Sync,
            > = Arc::new(move |event, _signal| {
                let event_tx = event_tx.clone();
                let done_tx = done_tx.clone();
                Box::pin(async move {
                    match &event {
                        pi_agent_core::types::AgentEvent::MessageUpdate {
                            assistant_message_event,
                            ..
                        } => {
                            if let pi_agent_core::pi_ai_types::AssistantMessageEvent::TextDelta {
                                delta,
                                ..
                            } = assistant_message_event
                            {
                                let _ = event_tx.send(serialize_json_line(&RpcOutput::Event {
                                    event: AgentEvent::MessageUpdate {
                                        delta: delta.clone(),
                                    },
                                }));
                            }
                        }
                        pi_agent_core::types::AgentEvent::MessageEnd { .. } => {
                            let _ = event_tx.send(serialize_json_line(&RpcOutput::Event {
                                event: AgentEvent::MessageEnd,
                            }));
                        }
                        pi_agent_core::types::AgentEvent::ToolExecutionStart {
                            tool_call_id,
                            tool_name,
                            args,
                            ..
                        } => {
                            let _ = event_tx.send(serialize_json_line(&RpcOutput::Event {
                                event: AgentEvent::ToolExecutionStart {
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: tool_name.clone(),
                                    args: args.clone(),
                                },
                            }));
                        }
                        pi_agent_core::types::AgentEvent::ToolExecutionEnd {
                            tool_call_id,
                            tool_name,
                            result,
                            is_error,
                            ..
                        } => {
                            let _ = event_tx.send(serialize_json_line(&RpcOutput::Event {
                                event: AgentEvent::ToolExecutionEnd {
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: tool_name.clone(),
                                    result: result.clone(),
                                    is_error: *is_error,
                                },
                            }));
                        }
                        pi_agent_core::types::AgentEvent::AgentEnd { .. } => {
                            let _ = event_tx.send(serialize_json_line(&RpcOutput::Event {
                                event: AgentEvent::AgentEnd,
                            }));
                            let _ = done_tx.send(false);
                        }
                        _ => {}
                    }
                })
            });

            session.subscribe(listener).await;
            session.add_user_text(&message).await;

            None
        }

        // ── Streaming Queue ──────────────────────────────────────────────

        RpcCommand::Steer { id, message, .. } => {
            session.steer(&message).await;
            Some(rpc_success(id, "steer", None))
        }

        RpcCommand::FollowUp { id, message, .. } => {
            session.follow_up(&message).await;
            Some(rpc_success(id, "follow_up", None))
        }

        // ── Abort ─────────────────────────────────────────────────────────

        RpcCommand::Abort { id } => {
            session.abort().await;
            Some(rpc_success(id, "abort", None))
        }

        RpcCommand::AbortBash { id } => {
            session.abort().await;
            Some(rpc_success(id, "abort_bash", None))
        }

        RpcCommand::Bash {
            id,
            command,
            exclude_from_context: _,
        } => {
            match session.execute_bash(&command).await {
                Ok(output) => Some(rpc_success(
                    id,
                    "bash",
                    Some(serde_json::json!({"status": "completed", "output": output})),
                )),
                Err(e) => Some(rpc_error(id, "bash", e)),
            }
        }

        // ── Session ──────────────────────────────────────────────────────

        RpcCommand::NewSession { id, parent_session } => {
            session.new_session(parent_session.as_deref()).await;
            Some(rpc_success(
                id,
                "new_session",
                Some(serde_json::json!({"cancelled": false})),
            ))
        }

        RpcCommand::GetState { id } => {
            let model = session.get_model().await;
            let thinking_level = session.get_thinking_level().await;
            let session_file = session.get_session_file().map(|p| p.to_string_lossy().to_string());
            let state_data = RpcSessionState {
                model: model.id.clone(),
                thinking_level,
                is_streaming: session.is_streaming().await,
                session_id: session.get_session_id(),
                session_name: session.get_session_name(),
                message_count: session.get_messages().await.len(),
                is_compacting: None, // Not yet tracked on AgentSession
                steering_mode: None, // Not yet implemented
                follow_up_mode: None, // Not yet implemented
                session_file,
                auto_compaction_enabled: Some(session.get_compaction_settings().compact_on_threshold),
                pending_message_count: Some(session.get_messages().await.len()),
            };
            Some(rpc_success(
                id,
                "get_state",
                Some(serde_json::to_value(state_data).unwrap_or_default()),
            ))
        }

        // ── Model ─────────────────────────────────────────────────────────

        RpcCommand::SetModel {
            id,
            provider,
            model_id,
        } => {
            let models = model_registry.get_available();
            let model = models.iter().find(|m| m.provider == provider && m.id == model_id).cloned();
            match model {
                Some(m) => {
                    session.set_model(m.clone()).await;
                    Some(rpc_success(
                        id,
                        "set_model",
                        Some(serde_json::json!({"id": m.id, "provider": m.provider})),
                    ))
                }
                None => Some(rpc_error(
                    id,
                    "set_model",
                    format!("Model not found: {provider}/{model_id}"),
                )),
            }
        }

        RpcCommand::CycleModel { id } => {
            let models = model_registry.get_available();
            if models.is_empty() {
                return Some(rpc_success(id, "cycle_model", None));
            }
            let current_id = session.get_model().await.id;
            let current_idx = models.iter().position(|m| m.id == current_id).unwrap_or(0);
            let next_idx = (current_idx + 1) % models.len();
            let next = &models[next_idx];
            session.set_model(next.clone()).await;
            Some(rpc_success(
                id,
                "cycle_model",
                Some(serde_json::json!({"model": {"id": next.id, "provider": next.provider}})),
            ))
        }

        RpcCommand::GetAvailableModels { id } => {
            let models = model_registry.get_available();
            let models_json: Vec<serde_json::Value> = models
                .iter()
                .map(|m| serde_json::json!({"id": m.id, "provider": m.provider}))
                .collect();
            Some(rpc_success(
                id,
                "get_available_models",
                Some(serde_json::json!({"models": models_json})),
            ))
        }

        // ── Thinking ──────────────────────────────────────────────────────

        RpcCommand::SetThinkingLevel { id, level } => {
            session.set_thinking_level(&level).await;
            Some(rpc_success(id, "set_thinking_level", Some(serde_json::json!({"level": level}))))
        }

        RpcCommand::CycleThinkingLevel { id } => {
            let levels = ["off", "minimal", "low", "medium", "high", "xhigh"];
            let current = session.get_thinking_level().await;
            let idx = levels.iter().position(|l| *l == current).unwrap_or(0);
            let next = levels[(idx + 1) % levels.len()];
            session.set_thinking_level(next).await;
            Some(rpc_success(
                id,
                "cycle_thinking_level",
                Some(serde_json::json!({"level": next})),
            ))
        }

        // ── Queue Modes ──────────────────────────────────────────────────

        RpcCommand::SetSteeringMode { id, mode: _ } => {
            // Steering mode is not yet implemented on AgentSession
            Some(rpc_success(id, "set_steering_mode", None))
        }

        RpcCommand::SetFollowUpMode { id, mode: _ } => {
            // Follow-up mode is not yet implemented on AgentSession
            Some(rpc_success(id, "set_follow_up_mode", None))
        }

        // ── Compaction ────────────────────────────────────────────────────

        RpcCommand::Compact {
            id,
            custom_instructions,
        } => {
            let result = session.compact(custom_instructions.as_deref()).await;
            match result {
                Ok(summary) => Some(rpc_success(
                    id,
                    "compact",
                    Some(serde_json::json!({"compacted": true, "summary": summary})),
                )),
                Err(reason) => Some(rpc_success(
                    id,
                    "compact",
                    Some(serde_json::json!({"compacted": false, "reason": reason})),
                )),
            }
        }

        RpcCommand::SetAutoCompaction { id, enabled } => {
            let mut settings = session.get_compaction_settings().clone();
            settings.compact_on_threshold = enabled;
            session.set_compaction_settings(settings);
            Some(rpc_success(id, "set_auto_compaction", Some(serde_json::json!({"enabled": enabled}))))
        }

        // ── Retry ─────────────────────────────────────────────────────────

        RpcCommand::SetAutoRetry { id, enabled: _ } => {
            // Auto-retry is not yet implemented on AgentSession
            Some(rpc_success(id, "set_auto_retry", None))
        }

        RpcCommand::AbortRetry { id } => {
            session.abort().await;
            Some(rpc_success(id, "abort_retry", None))
        }

        // ── Messages / Entries ───────────────────────────────────────────

        RpcCommand::GetMessages { id } => {
            let messages = session.get_messages().await;
            let messages_json: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| serde_json::to_value(m).unwrap_or_default())
                .collect();
            Some(rpc_success(
                id,
                "get_messages",
                Some(serde_json::json!({"messages": messages_json})),
            ))
        }

        RpcCommand::GetEntries { id, since } => {
            let (entry_ids, leaf_id) = {
                let mgr = session.get_session_manager();
                let ids: Vec<String> = mgr.get_entries().iter().map(|e| session_entry_id(e)).collect();
                (ids, mgr.get_session_id().to_string())
            };
            let all_entries: Vec<serde_json::Value> = entry_ids
                .iter()
                .map(|eid| serde_json::json!({"id": eid, "type": "entry"}))
                .collect();
            let entries = if let Some(ref since_id) = since {
                let since_idx = all_entries.iter().position(|e| {
                    e.get("id").and_then(|v| v.as_str()) == Some(since_id)
                });
                match since_idx {
                    Some(idx) => all_entries[idx + 1..].to_vec(),
                    None => {
                        return Some(rpc_error(id, "get_entries", format!("Entry not found: {since_id}")));
                    }
                }
            } else {
                all_entries
            };
            Some(rpc_success(
                id,
                "get_entries",
                Some(serde_json::json!({"entries": entries, "leafId": leaf_id})),
            ))
        }

        RpcCommand::GetTree { id } => {
            let (entry_ids, leaf_id) = {
                let mgr = session.get_session_manager();
                let ids: Vec<String> = mgr.get_entries().iter().map(|e| session_entry_id(e)).collect();
                (ids, mgr.get_session_id().to_string())
            };
            let entries: Vec<serde_json::Value> = entry_ids
                .iter()
                .map(|eid| serde_json::json!({"id": eid, "type": "entry"}))
                .collect();
            Some(rpc_success(
                id,
                "get_tree",
                Some(serde_json::json!({"tree": entries, "leafId": leaf_id})),
            ))
        }

        RpcCommand::SetSessionName { id, ref name } => {
            session.set_session_name(name);
            Some(rpc_success(id, "set_session_name", None))
        }

        // ── Session Stats ─────────────────────────────────────────────────

        RpcCommand::GetSessionStats { id } => {
            let stats = session.get_session_stats();
            Some(rpc_success(
                id,
                "get_session_stats",
                Some(serde_json::to_value(stats).unwrap_or_default()),
            ))
        }

        // ── Session Lifecycle ─────────────────────────────────────────────

        RpcCommand::SwitchSession { id, session_path } => {
            match session.switch_session(&session_path, None).await {
                Ok(()) => Some(rpc_success(id, "switch_session", Some(serde_json::json!({"cancelled": false})))),
                Err(e) => Some(rpc_error(id, "switch_session", e)),
            }
        }

        RpcCommand::Fork { id, entry_id } => {
            match session.fork_session(&entry_id).await {
                Ok(path) => Some(rpc_success(
                    id,
                    "fork",
                    Some(serde_json::json!({"path": path, "cancelled": false})),
                )),
                Err(e) => Some(rpc_error(id, "fork", e)),
            }
        }

        RpcCommand::Clone { id } => {
            let leaf_id = {
                let mgr = session.get_session_manager();
                mgr.get_leaf_id().map(|s| s.to_string())
            };
            match leaf_id {
                Some(eid) => match session.fork_session(&eid).await {
                    Ok(path) => Some(rpc_success(
                        id,
                        "clone",
                        Some(serde_json::json!({"path": path, "cancelled": false})),
                    )),
                    Err(e) => Some(rpc_error(id, "clone", e)),
                },
                None => Some(rpc_error(id, "clone", "Cannot clone session: no current entry selected".to_string())),
            }
        }

        RpcCommand::GetForkMessages { id } => {
            let messages = session.get_messages().await;
            let user_messages: Vec<serde_json::Value> = messages
                .iter()
                .filter_map(|m| {
                    if let pi_agent_core::types::AgentMessage::User { .. } = m {
                        Some(serde_json::to_value(m).unwrap_or_default())
                    } else {
                        None
                    }
                })
                .collect();
            Some(rpc_success(
                id,
                "get_fork_messages",
                Some(serde_json::json!({"messages": user_messages})),
            ))
        }

        RpcCommand::GetLastAssistantText { id } => {
            let text = session.get_last_assistant_text().await;
            Some(rpc_success(
                id,
                "get_last_assistant_text",
                Some(serde_json::json!({"text": text})),
            ))
        }

        RpcCommand::GetCommands { id } => {
            // Return available commands (slash commands, skills, prompt templates)
            // Currently returns built-in slash commands. Extension commands and
            // skills are not yet registered in the RPC handler.
            let commands: Vec<serde_json::Value> = vec![
                serde_json::json!({"name": "new", "description": "Start a new session"}),
                serde_json::json!({"name": "resume", "description": "Resume a previous session"}),
                serde_json::json!({"name": "fork", "description": "Fork the session at an entry"}),
                serde_json::json!({"name": "compact", "description": "Compact the session"}),
                serde_json::json!({"name": "help", "description": "Show help"}),
            ];
            Some(rpc_success(
                id,
                "get_commands",
                Some(serde_json::json!({"commands": commands})),
            ))
        }

        // ── Shutdown ─────────────────────────────────────────────────────

        RpcCommand::Shutdown { id } => {
            state.shutdown_requested = true;
            Some(rpc_success(id, "shutdown", None))
        }
    }
}

/// Extract the entry ID from any SessionEntry variant.
fn session_entry_id(entry: &SessionEntry) -> String {
    match entry {
        SessionEntry::Message { id, .. } => id.clone(),
        SessionEntry::ThinkingLevelChange { id, .. } => id.clone(),
        SessionEntry::ModelChange { id, .. } => id.clone(),
        SessionEntry::Compaction { id, .. } => id.clone(),
        SessionEntry::BranchSummary { id, .. } => id.clone(),
        SessionEntry::Custom { id, .. } => id.clone(),
        SessionEntry::CustomMessage { id, .. } => id.clone(),
        SessionEntry::Label { id, .. } => id.clone(),
        SessionEntry::SessionInfo { id, .. } => id.clone(),
    }
}

/// Shared state for the RPC handler.
pub struct RpcHandlerState {
    pub shutdown_requested: bool,
    pub event_tx: tokio::sync::mpsc::UnboundedSender<String>,
    pub error_tx: tokio::sync::mpsc::UnboundedSender<bool>,
    pub done_tx: tokio::sync::mpsc::UnboundedSender<bool>,
}

impl RpcHandlerState {
    pub fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let (error_tx, _error_rx) = tokio::sync::mpsc::unbounded_channel();
        let (done_tx, _done_rx) = tokio::sync::mpsc::unbounded_channel();

        (
            RpcHandlerState {
                shutdown_requested: false,
                event_tx,
                error_tx,
                done_tx,
            },
            event_rx,
        )
    }
}
