//! ACP `Agent` trait implementation for pi-coding-agent.
//!
//! Implements the agent side of the Agent Client Protocol so that ACP
//! clients (Zed, JetBrains, ACP VSCode extensions, …) can drive
//! pi-coding-agent directly over stdio.

use std::sync::Arc;

use agent_client_protocol as acp;
use futures::future::LocalBoxFuture;
use tokio::sync::{mpsc, oneshot};

use crate::config;

use super::session::{SessionCommand, SessionHandle, SessionRegistry};

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
            .mcp_capabilities(acp::McpCapabilities::new())
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
            .create(&cwd, self.notif_tx.clone(), &self.spawn)
            .await
            .map_err(internal_err)?;
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
        reg.load(&args.session_id, self.notif_tx.clone(), &self.spawn)
            .await
            .map_err(invalid_params_err)?;
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
                reply: reply_tx,
            })
            .map_err(|_| internal_err("session closed"))?;
        // Images are accepted by the session but not yet wired through ACP
        // (pi's prompt() takes images via PromptOptions); text-only for now.
        let _ = images;
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
                        level: value,
                        reply: reply_tx,
                    })
                    .map_err(|_| internal_err("session closed"))?;
                reply_rx
                    .await
                    .map_err(|_| internal_err("session closed"))?
                    .map_err(internal_err)?;
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
        // Return the full updated option set.
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(SessionCommand::GetConfigOptions { reply: reply_tx })
            .map_err(|_| internal_err("session closed"))?;
        let options = reply_rx
            .await
            .map_err(|_| internal_err("session closed"))?;
        Ok(acp::SetSessionConfigOptionResponse::new(options))
    }
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
