//! ACP `Agent` trait implementation for pi-coding-agent.
//!
//! Implements the agent side of the Agent Client Protocol so that ACP
//! clients (Zed, JetBrains, ACP VSCode extensions, …) can drive
//! pi-coding-agent directly over stdio.

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol as acp;
use futures::future::LocalBoxFuture;
use tokio::sync::{mpsc, oneshot};

use crate::config;

use super::session::{SessionCommand, SessionHandle, SessionRegistry};
use super::slash_commands::{builtin_available_commands, load_slash_commands, to_available_commands};

/// The ACP agent: owns the session registry and the notification channel.
pub struct PiAcpAgent {
    registry: Arc<tokio::sync::Mutex<SessionRegistry>>,
    notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
    spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)>,
}

impl PiAcpAgent {
    pub fn new(
        notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
        spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)>,
    ) -> Self {
        Self::with_base_dir(notif_tx, spawn, crate::config::get_sessions_dir())
    }

    /// Create the agent with a custom session-storage directory (used by tests
    /// to avoid writing to the real agent directory).
    pub fn with_base_dir(
        notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
        spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)>,
        base_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            registry: Arc::new(tokio::sync::Mutex::new(SessionRegistry::with_base_dir(
                base_dir,
            ))),
            notif_tx,
            spawn,
        }
    }

    async fn handle(&self, session_id: &acp::SessionId) -> Result<SessionHandle, acp::Error> {
        let reg = self.registry.lock().await;
        reg.get(session_id)
            .cloned()
            .ok_or_else(|| invalid_params_err("unknown session"))
    }
}

/// Build an ACP internal-error with a custom message.
fn internal_err(msg: impl Into<String>) -> acp::Error {
    acp::Error::new(acp::ErrorCode::InternalError.into(), msg)
}

/// Build an ACP invalid-params error with a custom message.
fn invalid_params_err(msg: impl Into<String>) -> acp::Error {
    acp::Error::new(acp::ErrorCode::InvalidParams.into(), msg)
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for PiAcpAgent {
    async fn initialize(
        &self,
        args: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        let _ = args;
        let capabilities = acp::AgentCapabilities::new()
            .load_session(true)
            .prompt_capabilities(acp::PromptCapabilities::new().image(true))
            // stdio MCP is always supported; streamable-HTTP only with the
            // `mcp` feature (which pulls the rmcp reqwest transport). SSE is
            // not implemented.
            .mcp_capabilities(acp::McpCapabilities::new().http(cfg!(feature = "mcp")))
            .session_capabilities(
                acp::SessionCapabilities::new().list(acp::SessionListCapabilities::new()),
            );
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(capabilities)
            .agent_info(
                acp::Implementation::new("pi-coding-agent", config::VERSION)
                    .title("Pi Coding Agent"),
            ))
    }

    async fn authenticate(
        &self,
        _args: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        // No auth methods are advertised, so clients never call this.
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        args: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        let cwd = args.cwd.to_string_lossy().to_string();
        let mut reg = self.registry.lock().await;
        let session_id = reg
            .create(&cwd, &args.mcp_servers, self.notif_tx.clone(), &self.spawn)
            .await
            .map_err(internal_err)?;

        // Startup info block (pi version + context), emitted as the first
        // chunk of the first prompt (mirrors pi-acp's startup info).
        let startup_info = build_startup_info(&cwd);
        if let Some(handle) = reg.get(&session_id) {
            let _ = handle.send(SessionCommand::SetStartupInfo { text: startup_info });
        }

        // Advertise slash commands after the response is delivered (clients
        // ignore notifications for unknown session IDs).
        self.emit_available_commands(session_id.clone(), &cwd);

        Ok(acp::NewSessionResponse::new(session_id))
    }

    async fn load_session(
        &self,
        args: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        // Load the session by ID, resuming its persisted messages. This works
        // across process restarts: sessions are recorded in the on-disk ACP
        // session map under `{sessions_dir}/acp/`.
        let mut reg = self.registry.lock().await;
        reg.load(&args.session_id, &args.mcp_servers, self.notif_tx.clone(), &self.spawn)
            .await
            .map_err(invalid_params_err)?;
        // Advertise slash commands after the response is delivered.
        let cwd = args.cwd.to_string_lossy().to_string();
        self.emit_available_commands(args.session_id.clone(), &cwd);
        Ok(acp::LoadSessionResponse::new())
    }

    async fn prompt(
        &self,
        args: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        let handle = self.handle(&args.session_id).await?;
        let (text, images) = extract_prompt(&args.prompt);
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(SessionCommand::Prompt {
                text,
                images,
                reply: reply_tx,
            })
            .map_err(|_| internal_err("session closed"))?;
        reply_rx
            .await
            .map_err(|_| internal_err("session closed"))?
            .map_err(internal_err)
    }

    async fn cancel(&self, args: acp::CancelNotification) -> Result<(), acp::Error> {
        let handle = self.handle(&args.session_id).await?;
        handle
            .send(SessionCommand::Cancel)
            .map_err(|_| internal_err("session closed"))
    }

    async fn list_sessions(
        &self,
        _args: acp::ListSessionsRequest,
    ) -> Result<acp::ListSessionsResponse, acp::Error> {
        let reg = self.registry.lock().await;
        Ok(acp::ListSessionsResponse::new(reg.list()))
    }

    async fn set_session_model(
        &self,
        args: acp::SetSessionModelRequest,
    ) -> Result<acp::SetSessionModelResponse, acp::Error> {
        let handle = self.handle(&args.session_id).await?;
        let model = resolve_model(&args.model_id.0).await?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(SessionCommand::SetModel {
                model,
                reply: reply_tx,
            })
            .map_err(|_| internal_err("session closed"))?;
        reply_rx
            .await
            .map_err(|_| internal_err("session closed"))?
            .map_err(internal_err)?;
        Ok(acp::SetSessionModelResponse::default())
    }

    async fn set_session_mode(
        &self,
        args: acp::SetSessionModeRequest,
    ) -> Result<acp::SetSessionModeResponse, acp::Error> {
        let handle = self.handle(&args.session_id).await?;
        let level = args.mode_id.0.to_string();
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(SessionCommand::SetThinkingLevel {
                level: level.clone(),
                reply: reply_tx,
            })
            .map_err(|_| internal_err("session closed"))?;
        reply_rx
            .await
            .map_err(|_| internal_err("session closed"))?
            .map_err(internal_err)?;
        // Keep the client's mode dropdown in sync.
        self.emit_notification(
            args.session_id.clone(),
            acp::SessionUpdate::CurrentModeUpdate(acp::CurrentModeUpdate::new(level)),
        );
        self.emit_config_options_update(args.session_id.clone()).await?;
        Ok(acp::SetSessionModeResponse::default())
    }

    async fn close_session(
        &self,
        args: acp::CloseSessionRequest,
    ) -> Result<acp::CloseSessionResponse, acp::Error> {
        let mut reg = self.registry.lock().await;
        reg.delete(&args.session_id);
        Ok(acp::CloseSessionResponse::default())
    }

    async fn set_session_config_option(
        &self,
        args: acp::SetSessionConfigOptionRequest,
    ) -> Result<acp::SetSessionConfigOptionResponse, acp::Error> {
        let handle = self.handle(&args.session_id).await?;
        let value = match &args.value {
            acp::SessionConfigOptionValue::ValueId { value } => value.0.to_string(),
            _ => {
                return Err(invalid_params_err("unsupported config value type"));
            }
        };
        match args.config_id.0.as_ref() {
            "thinking_level" => {
                let (reply_tx, reply_rx) = oneshot::channel();
                handle
                    .send(SessionCommand::SetThinkingLevel {
                        level: value.clone(),
                        reply: reply_tx,
                    })
                    .map_err(|_| internal_err("session closed"))?;
                reply_rx
                    .await
                    .map_err(|_| internal_err("session closed"))?
                    .map_err(internal_err)?;
                // Keep the client's mode dropdown in sync.
                self.emit_notification(
                    args.session_id.clone(),
                    acp::SessionUpdate::CurrentModeUpdate(acp::CurrentModeUpdate::new(value)),
                );
            }
            "model" => {
                let model = resolve_model(&value).await?;
                let (reply_tx, reply_rx) = oneshot::channel();
                handle
                    .send(SessionCommand::SetModel {
                        model,
                        reply: reply_tx,
                    })
                    .map_err(|_| internal_err("session closed"))?;
                reply_rx
                    .await
                    .map_err(|_| internal_err("session closed"))?
                    .map_err(internal_err)?;
            }
            _ => {
                return Err(invalid_params_err("unknown config option"));
            }
        }
        // Return the full updated option set and notify the client.
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(SessionCommand::GetConfigOptions { reply: reply_tx })
            .map_err(|_| internal_err("session closed"))?;
        let options = reply_rx
            .await
            .map_err(|_| internal_err("session closed"))?;
        self.emit_notification(
            args.session_id.clone(),
            acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(options.clone())),
        );
        Ok(acp::SetSessionConfigOptionResponse::new(options))
    }
}

impl PiAcpAgent {
    /// Send a session notification through the forwarding channel.
    fn emit_notification(&self, session_id: acp::SessionId, update: acp::SessionUpdate) {
        let notif = acp::SessionNotification::new(session_id, update);
        let (ack_tx, _ack_rx) = oneshot::channel();
        let _ = self.notif_tx.send((notif, ack_tx));
    }

    /// Emit a `config_options_update` notification for a session.
    async fn emit_config_options_update(&self, session_id: acp::SessionId) -> Result<(), acp::Error> {
        let handle = self.handle(&session_id).await?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(SessionCommand::GetConfigOptions { reply: reply_tx })
            .map_err(|_| internal_err("session closed"))?;
        let options = reply_rx
            .await
            .map_err(|_| internal_err("session closed"))?;
        self.emit_notification(
            session_id,
            acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(options)),
        );
        Ok(())
    }

    /// Advertise the available slash commands (file-based + built-in) for a
    /// session. The notification is emitted after a short delay so the client
    /// has already received the `session/new` / `session/load` response — some
    /// clients (e.g. Zed) drop notifications for unknown session IDs.
    fn emit_available_commands(&self, session_id: acp::SessionId, cwd: &str) {
        let file_commands = load_slash_commands(cwd);
        let mut commands = to_available_commands(&file_commands);
        commands.extend(builtin_available_commands());
        let notif = acp::SessionNotification::new(
            session_id,
            acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
                commands,
            )),
        );
        let tx = self.notif_tx.clone();
        let spawn = self.spawn.clone();
        spawn(Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let (ack_tx, _ack_rx) = oneshot::channel();
            let _ = tx.send((notif, ack_tx));
        }));
    }
}

/// Build the startup-info block emitted as the first chunk of the first
/// prompt (mirrors pi-acp's `buildStartupInfo` — pi version + context).
fn build_startup_info(cwd: &str) -> String {
    let mut md = Vec::new();
    md.push(format!("pi-coding-agent v{}", config::VERSION));
    md.push("---".to_string());
    md.push(String::new());
    md.push("## Context".to_string());
    md.push(format!("- cwd: {cwd}"));
    md.join("\n")
}

/// Extract plain text (and note images) from an ACP prompt's content blocks.
fn extract_prompt(prompt: &[acp::ContentBlock]) -> (String, Vec<pi_agent_core::pi_ai_types::ContentBlock>) {
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    for block in prompt {
        match block {
            acp::ContentBlock::Text(t) => text_parts.push(t.text.clone()),
            acp::ContentBlock::Image(img) => {
                images.push(pi_agent_core::pi_ai_types::ContentBlock::Image {
                    data: img.data.clone(),
                    mime_type: img.mime_type.clone(),
                });
            }
            _ => {}
        }
    }
    (text_parts.join("\n"), images)
}

/// Resolve a `provider/model` string to a pi `Model` via the session's model
/// registry.
async fn resolve_model(
    value: &str,
) -> Result<pi_agent_core::pi_ai_types::Model, acp::Error> {
    let (provider, model_id) = value
        .split_once('/')
        .ok_or_else(|| invalid_params_err("model must be provider/model"))?;
    // The registry is owned by the session task; resolve via a fresh lookup
    // using the builtin model list (same source the session uses).
    let models = crate::core::model_registry::ModelRegistry::builtin_models_list();
    let registry = crate::core::model_registry::ModelRegistry::new(models);
    registry
        .find(provider, model_id)
        .ok_or_else(|| invalid_params_err("unknown model"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_prompt_keeps_text_and_images() {
        let prompt = vec![
            acp::ContentBlock::Text(acp::TextContent::new("look at this")),
            acp::ContentBlock::Image(acp::ImageContent::new("base64data", "image/png")),
            acp::ContentBlock::Text(acp::TextContent::new("second")),
        ];
        let (text, images) = extract_prompt(&prompt);
        assert_eq!(text, "look at this\nsecond");
        assert_eq!(images.len(), 1);
        match &images[0] {
            pi_agent_core::pi_ai_types::ContentBlock::Image { data, mime_type } => {
                assert_eq!(data, "base64data");
                assert_eq!(mime_type, "image/png");
            }
            other => panic!("expected image block, got {other:?}"),
        }
    }

    #[test]
    fn extract_prompt_skips_non_text_non_image() {
        // An audio block (unsupported) is ignored; text is still collected.
        let prompt = vec![acp::ContentBlock::Text(acp::TextContent::new("hi"))];
        let (text, images) = extract_prompt(&prompt);
        assert_eq!(text, "hi");
        assert!(images.is_empty());
    }
}
