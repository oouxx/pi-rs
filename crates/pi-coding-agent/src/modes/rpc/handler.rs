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

        RpcCommand::Abort { id } => {
            session.abort().await;
            Some(rpc_success(id, "abort", None))
        }

        RpcCommand::AbortBash { id } => Some(rpc_success(id, "abort_bash", None)),

        RpcCommand::Bash {
            id,
            command,
            exclude_from_context: _,
        } => {
            session
                .add_user_text(&format!(
                    "Run this command and show me the output:\n```bash\n{command}\n```"
                ))
                .await;
            Some(rpc_success(
                id,
                "bash",
                Some(serde_json::json!({"status": "queued_as_prompt"})),
            ))
        }

        // ── Session ──────────────────────────────────────────────────────

        RpcCommand::NewSession {
            id,
            parent_session: _,
        } => Some(rpc_success(
            id,
            "new_session",
            Some(serde_json::json!({"cancelled": false})),
        )),

        RpcCommand::GetState { id } => {
            let model = session.get_model().await;
            let thinking_level = session.get_thinking_level().await;
            let state = RpcSessionState {
                model: model.id.clone(),
                thinking_level,
                is_streaming: session.is_streaming().await,
                session_id: session.get_session_id(),
                session_name: session.get_session_name(),
                message_count: session.get_messages().await.len(),
            };
            Some(rpc_success(
                id,
                "get_state",
                Some(serde_json::to_value(state).unwrap_or_default()),
            ))
        }

        // ── Model ────────────────────────────────────────────────────────

        RpcCommand::SetModel {
            id,
            provider,
            model_id,
        } => {
            let models = model_registry.get_available();
            let model = models.iter().find(|m| m.provider == provider && m.id == model_id).cloned();
            match model {
                Some(m) => Some(rpc_success(
                    id,
                    "set_model",
                    Some(serde_json::json!({"id": m.id, "provider": m.provider})),
                )),
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

        // ── Thinking ─────────────────────────────────────────────────────

        RpcCommand::SetThinkingLevel { id, level: _ } => {
            Some(rpc_success(id, "set_thinking_level", None))
        }

        RpcCommand::Compact {
            id,
            custom_instructions: _,
        } => {
            let result =
                serde_json::json!({"compacted": false, "reason": "compaction not fully implemented"});
            Some(rpc_success(id, "compact", Some(result)))
        }

        RpcCommand::SetAutoCompaction { id, enabled: _ } => {
            // Compaction settings don't have auto_compaction yet
            Some(rpc_success(id, "set_auto_compaction", None))
        }

        // ── Messages / Entries ───────────────────────────────────────────

        RpcCommand::GetMessages { id } => {
            Some(rpc_success(
                id,
                "get_messages",
                Some(serde_json::json!({"messages": []})),
            ))
        }

        RpcCommand::GetEntries { id, since: _ } => {
            let session_manager = session.get_session_manager();
            let entries: Vec<serde_json::Value> = session_manager
                .get_entries()
                .iter()
                .map(|e| {
                    let entry_id = session_entry_id(e);
                    serde_json::json!({"id": entry_id, "type": "entry"})
                })
                .collect();
            let leaf_id = session_manager.get_session_id();
            Some(rpc_success(
                id,
                "get_entries",
                Some(serde_json::json!({"entries": entries, "leafId": leaf_id})),
            ))
        }

        RpcCommand::GetTree { id } => {
            let session_manager = session.get_session_manager();
            let entries: Vec<serde_json::Value> = session_manager
                .get_entries()
                .iter()
                .map(|e| {
                    let entry_id = session_entry_id(e);
                    serde_json::json!({"id": entry_id, "type": "entry"})
                })
                .collect();
            Some(rpc_success(
                id,
                "get_tree",
                Some(serde_json::json!({"tree": entries, "leafId": session_manager.get_session_id()})),
            ))
        }

        RpcCommand::SetSessionName { id, ref name } => {
            session.set_session_name(name);
            Some(rpc_success(id, "set_session_name", None))
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
