//! RPC command handler — dispatches RPC commands to AgentSession.
//!
//! Mirrors the command handling in packages/coding-agent/src/modes/rpc/rpc-mode.ts

use std::pin::Pin;
/// Agent event listener used by the RPC handler.
type RpcAgentEventListener = Arc<
    dyn Fn(
            pi_agent_core::types::AgentEvent,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::agent_session::AgentSession;
use crate::core::model_registry::ModelRegistry;

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
            images,
            streaming_behavior,
        } => {
            let output_tx = state.output_tx.clone();
            let cmd_id = id.clone();

            // Use a shared cell so the listener can unsubscribe itself after firing,
            // preventing duplicate responses when multiple prompt commands are sent.
            let unsubscribe_handle = Arc::new(tokio::sync::Mutex::new(None::<pi_agent_core::agent::UnsubscribeHandle>));
            let uh = unsubscribe_handle.clone();

            let listener: RpcAgentEventListener = Arc::new(move |event, _signal| {
                let output_tx = output_tx.clone();
                let cmd_id = cmd_id.clone();
                let uh = uh.clone();
                Box::pin(async move {
                    // Events are forwarded globally via subscribe_session_events in mod.rs.
                    // This listener only handles the prompt success response.
                    if matches!(event, pi_agent_core::types::AgentEvent::AgentEnd { .. }) {
                        // Emit success response for prompt command
                        // (matching TS preflightResult pattern)
                        let success = rpc_success(cmd_id, "prompt", None);
                        let _ = output_tx.send(serialize_json_line(&success));
                        // Unsubscribe to prevent firing again on subsequent AgentEnd events
                        if let Some(handle) = uh.lock().await.take() {
                            handle.unsubscribe().await;
                        }
                    }
                })
            });

            // Build PromptOptions matching TS prompt() call
            let images_content = images.map(|imgs| {
                imgs.into_iter()
                    .filter_map(|img| {
                        let data = img.data?;
                        let mime_type = img.mime_type.unwrap_or_else(|| "image/png".to_string());
                        Some(pi_agent_core::pi_ai_types::ContentBlock::Image {
                            data,
                            mime_type,
                        })
                    })
                    .collect::<Vec<_>>()
            });

            let options = crate::core::agent_session::PromptOptions {
                expand_prompt_templates: None,
                images: images_content.filter(|v| !v.is_empty()),
                streaming_behavior,
                source: Some("rpc".to_string()),
            };

            // Track whether AgentEnd was received (agent run completed successfully)
            let agent_end_received = Arc::new(AtomicBool::new(false));
            let aer = agent_end_received.clone();

            // Wrap the listener to also set the flag on AgentEnd
            let original_listener = listener;
            let listener: RpcAgentEventListener = Arc::new(move |event, signal| {
                let aer = aer.clone();
                let orig = original_listener.clone();
                Box::pin(async move {
                    if matches!(event, pi_agent_core::types::AgentEvent::AgentEnd { .. }) {
                        aer.store(true, Ordering::SeqCst);
                    }
                    orig(event, signal).await;
                })
            });

            let handle = session.get_agent().subscribe(listener).await;
            *unsubscribe_handle.lock().await = Some(handle);

            // Call prompt and let the listener handle events + success response.
            // prompt() returns Err when the run cannot start (no model / no API
            // key) — propagate the real error, matching TS
            // session.prompt().catch((e) => output(error(id, "prompt", e.message))).
            match session.prompt(&message, Some(options)).await {
                Ok(()) => {
                    // After prompt() returns, check if AgentEnd was received.
                    // If not, the agent run failed to start (e.g. agent was already busy),
                    // and the RPC client would hang waiting for a response.
                    if !agent_end_received.load(Ordering::SeqCst) {
                        let err = rpc_error(id, "prompt", "Agent run failed to start or completed without emitting AgentEnd".to_string());
                        let _ = state.output_tx.send(serialize_json_line(&err));
                    }
                }
                Err(e) => {
                    let err = rpc_error(id, "prompt", e);
                    let _ = state.output_tx.send(serialize_json_line(&err));
                }
            }

            None
        }

        // ── Streaming Queue ──────────────────────────────────────────────

        RpcCommand::Steer { id, message, images } => {
            let images_content = images.map(|imgs| {
                imgs.into_iter()
                    .filter_map(|img| {
                        let data = img.data?;
                        let mime_type = img.mime_type.unwrap_or_else(|| "image/png".to_string());
                        Some(pi_agent_core::pi_ai_types::ContentBlock::Image {
                            data,
                            mime_type,
                        })
                    })
                    .collect::<Vec<_>>()
            });
            session.steer(&message, images_content.filter(|v| !v.is_empty())).await;
            Some(rpc_success(id, "steer", None))
        }

        RpcCommand::FollowUp { id, message, images } => {
            let images_content = images.map(|imgs| {
                imgs.into_iter()
                    .filter_map(|img| {
                        let data = img.data?;
                        let mime_type = img.mime_type.unwrap_or_else(|| "image/png".to_string());
                        Some(pi_agent_core::pi_ai_types::ContentBlock::Image {
                            data,
                            mime_type,
                        })
                    })
                    .collect::<Vec<_>>()
            });
            session.follow_up(&message, images_content.filter(|v| !v.is_empty())).await;
            Some(rpc_success(id, "follow_up", None))
        }

        // ── Abort ─────────────────────────────────────────────────────────

        RpcCommand::Abort { id } => {
            session.abort().await;
            Some(rpc_success(id, "abort", None))
        }

        RpcCommand::AbortBash { id } => {
            session.abort_bash();
            Some(rpc_success(id, "abort_bash", None))
        }

        RpcCommand::Bash {
            id,
            command,
            exclude_from_context,
        } => {
            match session
                .execute_bash(&command, None, exclude_from_context, id.clone())
                .await
            {
                Ok(result) => Some(rpc_success(
                    id,
                    "bash",
                    Some(serde_json::to_value(result).unwrap_or_default()),
                )),
                Err(e) => Some(rpc_error(id, "bash", e)),
            }
        }

        // ── Session ──────────────────────────────────────────────────────

        RpcCommand::NewSession { id, parent_session } => {
            session.session_mgr_new(parent_session.as_deref()).await;
            // Note: TS calls rebindSession() after new_session to re-subscribe
            // events and re-bind extensions. In the current Rust architecture,
            // extensions are bound at construction time, so rebinding is a no-op.
            // The session reference remains valid after new_session().
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
            let is_compacting = session.is_compacting();
            let steering_mode = session.steering_mode().await;
            let follow_up_mode = session.follow_up_mode().await;
            let auto_compaction = session.auto_compaction_enabled();
            let pending_count = session.pending_message_count();
            let messages = session.get_messages().await;

            let steering_mode_str = match steering_mode {
                pi_agent_core::types::QueueMode::All => "all",
                pi_agent_core::types::QueueMode::OneAtATime => "one-at-a-time",
            };
            let follow_up_mode_str = match follow_up_mode {
                pi_agent_core::types::QueueMode::All => "all",
                pi_agent_core::types::QueueMode::OneAtATime => "one-at-a-time",
            };

            let state_data = RpcSessionState {
                model: model.clone(),
                thinking_level,
                is_streaming: session.is_streaming().await,
                session_id: session.get_session_id(),
                session_name: session.get_session_name(),
                message_count: messages.len(),
                is_compacting: Some(is_compacting),
                steering_mode: Some(steering_mode_str.to_string()),
                follow_up_mode: Some(follow_up_mode_str.to_string()),
                session_file,
                auto_compaction_enabled: Some(auto_compaction),
                pending_message_count: Some(pending_count),
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
                    match session.set_model(m.clone()).await {
                        Ok(_) => Some(rpc_success(
                            id,
                            "set_model",
                            Some(serde_json::to_value(&m).unwrap_or_default()),
                        )),
                        Err(e) => Some(rpc_error(
                            id,
                            "set_model",
                            format!("Auth error: {e}"),
                        )),
                    }
                }
                None => Some(rpc_error(
                    id,
                    "set_model",
                    format!("Model not found: {provider}/{model_id}"),
                )),
            }
        }

        RpcCommand::CycleModel { id } => {
            let result = session.cycle_model("forward").await;
            match result {
                Some((model, thinking_level, is_scoped)) => {
                    Some(rpc_success(
                        id,
                        "cycle_model",
                        Some(serde_json::json!({
                            "model": model,
                            "thinkingLevel": thinking_level,
                            "isScoped": is_scoped,
                        })),
                    ))
                }
                None => {
                    Some(rpc_success(id, "cycle_model", None))
                }
            }
        }

        RpcCommand::GetAvailableModels { id } => {
            let models = model_registry.get_available();
            let models_json: Vec<serde_json::Value> = models
                .iter()
                .map(|m| serde_json::to_value(m).unwrap_or_default())
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
            Some(rpc_success(id, "set_thinking_level", None))
        }

        RpcCommand::CycleThinkingLevel { id } => {
            let result = session.cycle_thinking_level().await;
            match result {
                Some(level) => Some(rpc_success(
                    id,
                    "cycle_thinking_level",
                    Some(serde_json::json!({"level": level})),
                )),
                None => Some(rpc_success(id, "cycle_thinking_level", None)),
            }
        }

        RpcCommand::GetAvailableThinkingLevels { id } => {
            let levels = session.get_available_thinking_levels().await;
            Some(rpc_success(
                id,
                "get_available_thinking_levels",
                Some(serde_json::json!({"levels": levels})),
            ))
        }

        // ── Queue Modes ──────────────────────────────────────────────────

        RpcCommand::SetSteeringMode { id, mode } => {
            let queue_mode = match mode.as_str() {
                "one-at-a-time" => pi_agent_core::types::QueueMode::OneAtATime,
                _ => pi_agent_core::types::QueueMode::All,
            };
            session.set_steering_mode(queue_mode).await;
            Some(rpc_success(id, "set_steering_mode", None))
        }

        RpcCommand::SetFollowUpMode { id, mode } => {
            let queue_mode = match mode.as_str() {
                "one-at-a-time" => pi_agent_core::types::QueueMode::OneAtATime,
                _ => pi_agent_core::types::QueueMode::All,
            };
            session.set_follow_up_mode(queue_mode).await;
            Some(rpc_success(id, "set_follow_up_mode", None))
        }

        // ── Compaction ────────────────────────────────────────────────────

        RpcCommand::Compact {
            id,
            custom_instructions,
        } => {
            let result = session.compact(custom_instructions.as_deref()).await;
            match result {
                Ok(compact_result) => Some(rpc_success(
                    id,
                    "compact",
                    Some(serde_json::to_value(compact_result).unwrap_or_default()),
                )),
                Err(reason) => Some(rpc_error(
                    id,
                    "compact",
                    reason,
                )),
            }
        }

        RpcCommand::SetAutoCompaction { id, enabled } => {
            session.set_auto_compaction_enabled(enabled);
            Some(rpc_success(id, "set_auto_compaction", None))
        }

        // ── Retry ─────────────────────────────────────────────────────────

        RpcCommand::SetAutoRetry { id, enabled } => {
            session.set_auto_retry_enabled(enabled);
            Some(rpc_success(id, "set_auto_retry", None))
        }

        RpcCommand::AbortRetry { id } => {
            session.abort_retry();
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
            let (entries, leaf_id) = {
                let mgr = session.get_session_manager();
                let entries: Vec<serde_json::Value> = mgr
                    .get_entries()
                    .iter()
                    .map(|e| serde_json::to_value(e).unwrap_or_default())
                    .collect();
                let leaf_id = mgr.get_leaf_id().map(|s| s.to_string());
                (entries, leaf_id)
            };
            let result_entries = if let Some(ref since_id) = since {
                let since_idx = entries.iter().position(|e| {
                    e.get("id").and_then(|v| v.as_str()) == Some(since_id)
                });
                match since_idx {
                    Some(idx) => entries[idx + 1..].to_vec(),
                    None => {
                        return Some(rpc_error(id, "get_entries", format!("Entry not found: {since_id}")));
                    }
                }
            } else {
                entries
            };
            Some(rpc_success(
                id,
                "get_entries",
                Some(serde_json::json!({"entries": result_entries, "leafId": leaf_id})),
            ))
        }

        RpcCommand::GetTree { id } => {
            let (tree, leaf_id) = {
                let mgr = session.get_session_manager();
                let tree = mgr.get_tree();
                let leaf_id = mgr.get_leaf_id().map(|s| s.to_string());
                (tree, leaf_id)
            };
            // Serialize tree nodes manually since SessionTreeNode doesn't derive Serialize
            let tree_json: Vec<serde_json::Value> = tree
                .into_iter()
                .map(|node| serialize_tree_node(&node))
                .collect();
            Some(rpc_success(
                id,
                "get_tree",
                Some(serde_json::json!({"tree": tree_json, "leafId": leaf_id})),
            ))
        }

        RpcCommand::SetSessionName { id, ref name } => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return Some(rpc_error(id, "set_session_name", "Session name cannot be empty".to_string()));
            }
            session.set_session_name(trimmed);
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
            match session.session_mgr_switch(&session_path, None).await {
                Ok(()) => Some(rpc_success(id, "switch_session", Some(serde_json::json!({"cancelled": false})))),
                Err(e) => Some(rpc_error(id, "switch_session", e)),
            }
        }

        RpcCommand::Fork { id, entry_id } => {
            match session.session_mgr_fork(&entry_id).await {
                Ok(_path) => Some(rpc_success(
                    id,
                    "fork",
                    Some(serde_json::json!({"text": "", "cancelled": false})),
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
                Some(eid) => match session.session_mgr_fork(&eid).await {
                    Ok(_path) => Some(rpc_success(
                        id,
                        "clone",
                        Some(serde_json::json!({"cancelled": false})),
                    )),
                    Err(e) => Some(rpc_error(id, "clone", e)),
                },
                None => Some(rpc_error(id, "clone", "Cannot clone session: no current entry selected".to_string())),
            }
        }

        RpcCommand::GetForkMessages { id } => {
            let messages = session.get_user_messages_for_forking();
            let result: Vec<serde_json::Value> = messages
                .into_iter()
                .map(|(entry_id, text)| {
                    serde_json::json!({"entryId": entry_id, "text": text})
                })
                .collect();
            Some(rpc_success(
                id,
                "get_fork_messages",
                Some(serde_json::json!({"messages": result})),
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
            // Build the list of discoverable slash commands (the TS
            // `getCommands()`), reusing the typed `SlashCommandInfo` so the
            // wire format stays in sync with `slash-commands.ts`.
            //
            // Order matches TS: extension commands first, then prompt
            // templates, then skills.
            let mut commands: Vec<crate::core::slash_commands::SlashCommandInfo> = Vec::new();

            // Extension commands → source = "extension"
            // Uses resolve_extension_commands to apply the `:N` dedup logic
            // that TS `getRegisteredCommands()` performs.
            if let Some(registry) = session.get_extension_registry() {
                let resolved =
                    crate::core::slash_commands::resolve_extension_commands(registry.commands());
                for cmd in resolved {
                    commands.push(crate::core::slash_commands::SlashCommandInfo {
                        name: cmd.invocation_name,
                        description: cmd.description,
                        source: crate::core::slash_commands::SlashCommandSource::Extension,
                        source_info: cmd.source_info,
                    });
                }
            }

            // Prompt templates → source = "prompt"
            for template in session.prompt_templates() {
                commands.push(crate::core::slash_commands::SlashCommandInfo {
                    name: template.name,
                    description: Some(template.description),
                    source: crate::core::slash_commands::SlashCommandSource::Prompt,
                    source_info: template.source_info,
                });
            }

            // Skills → name = "skill:<name>", source = "skill"
            if let Some(resources) = session.resource_loader() {
                for skill in &resources.skills {
                    commands.push(crate::core::slash_commands::SlashCommandInfo {
                        name: format!("skill:{}", skill.name),
                        description: Some(skill.description.clone()),
                        source: crate::core::slash_commands::SlashCommandSource::Skill,
                        source_info: skill.source_info.clone(),
                    });
                }
            }

            Some(rpc_success(
                id,
                "get_commands",
                Some(serde_json::json!({ "commands": commands })),
            ))
        }

        // ── Export HTML ───────────────────────────────────────────────────

        RpcCommand::ExportHtml { id, output_path } => {
            match session.export_html_to_file(output_path.as_deref()) {
                Ok(path) => Some(rpc_success(
                    id,
                    "export_html",
                    Some(serde_json::json!({"path": path})),
                )),
                Err(e) => Some(rpc_error(id, "export_html", e)),
            }
        }

        // ── Shutdown ─────────────────────────────────────────────────────

        RpcCommand::Shutdown { id } => {
            state.shutdown_requested = true;
            Some(rpc_success(id, "shutdown", None))
        }
    }
}

/// Serialize a SessionTreeNode to a JSON value.
fn serialize_tree_node(node: &crate::core::session_manager::SessionTreeNode) -> serde_json::Value {
    let entry = serde_json::to_value(&node.entry).unwrap_or_default();
    let children: Vec<serde_json::Value> = node
        .children
        .iter()
        .map(serialize_tree_node)
        .collect();
    let mut map = serde_json::Map::new();
    map.insert("entry".to_string(), entry);
    map.insert("children".to_string(), serde_json::Value::Array(children));
    if let Some(ref label) = node.label {
        map.insert("label".to_string(), serde_json::Value::String(label.clone()));
    }
    if let Some(ref label_ts) = node.label_timestamp {
        map.insert("labelTimestamp".to_string(), serde_json::Value::String(label_ts.clone()));
    }
    serde_json::Value::Object(map)
}

/// Shared state for the RPC handler.
pub struct RpcHandlerState {
    pub shutdown_requested: bool,
    pub output_tx: tokio::sync::mpsc::UnboundedSender<String>,
    /// Pending extension UI requests waiting for client response.
    /// Maps request ID to a oneshot sender for the response value.
    pub pending_extension_requests:
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
}

impl RpcHandlerState {
    pub fn new(
        output_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Self {
        RpcHandlerState {
            shutdown_requested: false,
            output_tx,
            pending_extension_requests: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        }
    }
}
