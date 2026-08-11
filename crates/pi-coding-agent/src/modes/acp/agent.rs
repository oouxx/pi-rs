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
    /// Most recent session cwd, used as the default `session/list` filter
    /// (matching pi-acp's `lastSessionCwd` — Zed sends `{}` and expects the
    /// project-scoped list).
    last_session_cwd: std::sync::Mutex<Option<String>>,
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
            last_session_cwd: std::sync::Mutex::new(None),
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
                acp::SessionCapabilities::new()
                    .list(acp::SessionListCapabilities::new())
                    // `session/close` is implemented (matching pi-acp's
                    // `delete` capability advertisement).
                    .close(acp::SessionCloseCapabilities::new()),
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
        // ACP requires an absolute cwd (matching pi-acp's validation).
        if !args.cwd.is_absolute() {
            return Err(invalid_params_err(format!(
                "cwd must be an absolute path: {}",
                args.cwd.display()
            )));
        }
        let cwd = args.cwd.to_string_lossy().to_string();
        *self.last_session_cwd.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(cwd.clone());
        let session_id = {
            let mut reg = self.registry.lock().await;
            reg.create(&cwd, &args.mcp_servers, self.notif_tx.clone(), &self.spawn)
                .await
                .map_err(internal_err)?
        };

        // Startup info block (pi version + context), emitted as the first
        // chunk of the first prompt (mirrors pi-acp's startup info).
        let startup_info = build_startup_info(&cwd);
        if let Ok(handle) = self.handle(&session_id).await {
            let _ = handle.send(SessionCommand::SetStartupInfo { text: startup_info });
        }

        // Advertise slash commands after the response is delivered (clients
        // ignore notifications for unknown session IDs).
        self.emit_available_commands(session_id.clone(), &cwd).await;

        // Return the initial config options (model + thinking level) so the
        // client can render the dropdowns immediately (matching pi-acp's
        // `newSession` response).
        let config_options = self
            .config_options_for(&session_id)
            .await
            .unwrap_or_default();
        Ok(acp::NewSessionResponse::new(session_id).config_options(config_options))
    }

    async fn load_session(
        &self,
        args: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        // ACP requires an absolute cwd (matching pi-acp's validation).
        if !args.cwd.is_absolute() {
            return Err(invalid_params_err(format!(
                "cwd must be an absolute path: {}",
                args.cwd.display()
            )));
        }
        let cwd = args.cwd.to_string_lossy().to_string();
        *self.last_session_cwd.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(cwd.clone());
        // Load the session by ID, resuming its persisted messages. This works
        // across process restarts: sessions are recorded in the on-disk ACP
        // session map under `{sessions_dir}/acp/`.
        {
            let mut reg = self.registry.lock().await;
            reg.load(
                &args.session_id,
                &args.mcp_servers,
                self.notif_tx.clone(),
                &self.spawn,
                Some(&cwd),
            )
            .await
            .map_err(invalid_params_err)?;
        }
        // Advertise slash commands after the response is delivered.
        self.emit_available_commands(args.session_id.clone(), &cwd).await;
        // Return the initial config options (matching pi-acp's `loadSession`
        // response).
        let config_options = self
            .config_options_for(&args.session_id)
            .await
            .unwrap_or_default();
        Ok(acp::LoadSessionResponse::new().config_options(config_options))
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
        // Cancel is a notification: an unknown session is a silent no-op
        // (matching pi-acp's `maybeGet` guard).
        let Ok(handle) = self.handle(&args.session_id).await else {
            return Ok(());
        };
        handle
            .send(SessionCommand::Cancel)
            .map_err(|_| internal_err("session closed"))
    }

    async fn list_sessions(
        &self,
        args: acp::ListSessionsRequest,
    ) -> Result<acp::ListSessionsResponse, acp::Error> {
        // ACP: filter by cwd if provided. Zed sends `{}` (no cwd), so default
        // to the last session cwd to emulate pi's `/resume` picker
        // (project-scoped), matching pi-acp.
        let effective_cwd: Option<String> = if args.cwd.is_some() {
            args.cwd
                .clone()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            self.last_session_cwd
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };

        // All pi sessions under this agent's storage root (ACP-created ones
        // live under `{base_dir}/acp/`, regular CLI sessions under
        // `{base_dir}/<cwd>/…`), matching pi-acp's `listPiSessions` recursive
        // walk. Scoped to the registry's base dir so tests stay isolated.
        // Registry-tracked sessions (in-memory + persisted map) are merged in
        // so a just-created session whose JSONL header hasn't been flushed yet
        // still appears.
        let mut by_id = {
            let reg = self.registry.lock().await;
            let mut by_id: std::collections::HashMap<String, super::session::PiSessionItem> =
                super::session::scan_pi_sessions(reg.base_dir())
                    .into_iter()
                    .map(|s| (s.session_id.clone(), s))
                    .collect();
            for info in reg.list() {
                by_id
                    .entry(info.session_id.0.to_string())
                    .or_insert_with(|| super::session::PiSessionItem {
                        session_id: info.session_id.0.to_string(),
                        cwd: info.cwd.to_string_lossy().to_string(),
                        title: info.title.clone(),
                        updated_at: info.updated_at.clone(),
                    });
            }
            by_id
        };
        // Sort most recent first (matching pi-acp).
        let mut all: Vec<super::session::PiSessionItem> = by_id.drain().map(|(_, v)| v).collect();
        all.sort_by(|a, b| {
            let aa = a.updated_at.as_deref().unwrap_or("");
            let bb = b.updated_at.as_deref().unwrap_or("");
            bb.cmp(aa)
        });
        let filtered: Vec<_> = match &effective_cwd {
            Some(cwd) => all.into_iter().filter(|s| s.cwd == *cwd).collect(),
            None => all,
        };

        // Cursor-based pagination (opaque cursor; numeric offset, matching
        // pi-acp). Invalid cursor → 0.
        let offset = args
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        const PAGE_SIZE: usize = 50;
        let page = filtered.iter().skip(offset).take(PAGE_SIZE);
        let sessions: Vec<acp::SessionInfo> = page
            .map(|s| {
                let mut info =
                    acp::SessionInfo::new(s.session_id.clone(), std::path::PathBuf::from(&s.cwd));
                if let Some(title) = &s.title {
                    info = info.title(title.clone());
                }
                if let Some(updated_at) = &s.updated_at {
                    info = info.updated_at(updated_at.clone());
                }
                info
            })
            .collect();
        let next_cursor = (offset + PAGE_SIZE < filtered.len())
            .then(|| (offset + PAGE_SIZE).to_string());
        Ok(acp::ListSessionsResponse::new(sessions).next_cursor(next_cursor))
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
        // Keep the client's model dropdown in sync (matching pi-acp's
        // `emitConfigOptionsUpdate` after `setSessionModel`).
        self.emit_config_options_update(args.session_id.clone()).await?;
        Ok(acp::SetSessionModelResponse::default())
    }

    async fn set_session_mode(
        &self,
        args: acp::SetSessionModeRequest,
    ) -> Result<acp::SetSessionModeResponse, acp::Error> {
        let level = args.mode_id.0.to_string();
        // Validate the thinking level (matching pi-acp's `isThinkingLevel`).
        if !is_thinking_level(&level) {
            return Err(invalid_params_err(format!("Unknown modeId: {level}")));
        }
        let handle = self.handle(&args.session_id).await?;
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
            // `thought_level` is the pi-acp config id; `thinking_level` is the
            // legacy spelling this implementation previously advertised.
            "thought_level" | "thinking_level" => {
                if !is_thinking_level(&value) {
                    return Err(invalid_params_err(format!(
                        "Unknown thinking level: {value}"
                    )));
                }
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

    /// Fetch the current config options from a session task (used to populate
    /// `session/new` / `session/load` responses).
    async fn config_options_for(
        &self,
        session_id: &acp::SessionId,
    ) -> Result<Vec<acp::SessionConfigOption>, acp::Error> {
        let handle = self.handle(session_id).await?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(SessionCommand::GetConfigOptions { reply: reply_tx })
            .map_err(|_| internal_err("session closed"))?;
        reply_rx
            .await
            .map_err(|_| internal_err("session closed"))
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

    /// Advertise the available slash commands for a session. The list is built
    /// by the session task from pi's own discoverable commands (prompt
    /// templates + skills) + file-based + built-ins, matching pi-acp's
    /// `get_commands` + `mergeCommands`. The notification is emitted after a
    /// short delay so the client has already received the `session/new` /
    /// `session/load` response — some clients (e.g. Zed) drop notifications
    /// for unknown session IDs.
    async fn emit_available_commands(&self, session_id: acp::SessionId, cwd: &str) {
        let tx = self.notif_tx.clone();
        let spawn = self.spawn.clone();
        let cwd_owned = cwd.to_string();
        // Resolve the handle up-front so the spawned future is 'static.
        let handle = self.handle(&session_id).await.ok();
        spawn(Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            // Prefer the session task's command list (get_commands); fall back
            // to file-based + built-in if the session is busy (matching
            // pi-acp's try/catch fallback).
            let commands = match &handle {
                Some(h) => {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    if h.send(SessionCommand::GetCommands { reply: reply_tx }).is_ok() {
                        reply_rx.await.unwrap_or_default()
                    } else {
                        fallback_commands(&cwd_owned)
                    }
                }
                None => fallback_commands(&cwd_owned),
            };
            let notif = acp::SessionNotification::new(
                session_id,
                acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
                    commands,
                )),
            );
            let (ack_tx, _ack_rx) = oneshot::channel();
            let _ = tx.send((notif, ack_tx));
        }));
    }
}

/// Fallback available-commands list (file-based + built-in) used when the
/// session task is busy (matching pi-acp's fallback path).
fn fallback_commands(cwd: &str) -> Vec<acp::AvailableCommand> {
    let file_commands = load_slash_commands(cwd);
    let mut commands = to_available_commands(&file_commands);
    commands.extend(builtin_available_commands());
    commands
}

/// Build the startup-info block emitted as the first chunk of the first
/// prompt (mirrors pi-acp's `buildStartupInfo` — pi version + context +
/// skills + prompts + extensions).
fn build_startup_info(cwd: &str) -> String {
    let mut md = Vec::new();
    md.push(format!("pi-coding-agent v{}", config::VERSION));
    md.push("---".to_string());
    md.push(String::new());

    let mut add_section = |title: &str, items: Vec<String>| {
        let cleaned: Vec<String> = items.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if cleaned.is_empty() {
            return;
        }
        md.push(format!("## {title}"));
        for item in cleaned {
            md.push(format!("- {item}"));
        }
        md.push(String::new());
    };

    // Context
    let context_path = std::path::Path::new(cwd).join("AGENTS.md");
    let context_items = if context_path.exists() {
        vec![context_path.to_string_lossy().to_string()]
    } else {
        vec![]
    };
    add_section("Context", context_items);

    // Skills (global + project, matching pi-acp's discovery)
    let mut skills = Vec::new();
    for root in [
        config::get_agent_dir().join("skills"),
        std::path::Path::new(cwd).join(".pi").join("skills"),
    ] {
        collect_skill_md(&root, &mut skills);
    }
    add_section("Skills", skills);

    // Prompts (user prompt templates)
    let prompts_dir = config::get_agent_dir().join("prompts");
    let mut prompts = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&prompts_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                prompts.push(format!("/{}", name.trim_end_matches(".md")));
            }
        }
    }
    add_section("Prompts", prompts);

    // Extensions (user extension files)
    let ext_dir = config::get_agent_dir().join("extensions");
    let mut exts = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&ext_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".ts") || name.ends_with(".js") {
                exts.push(entry.path().to_string_lossy().to_string());
            }
        }
    }
    add_section("Extensions", exts);

    md.join("\n").trim_end().to_string() + "\n"
}

/// Recursively collect `SKILL.md` files (and root-level `.md` files) under a
/// skills directory, matching pi-acp's `pushSkillFromRoot`.
fn collect_skill_md(root: &std::path::Path, out: &mut Vec<String>) {
    if !root.is_dir() {
        return;
    }
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "node_modules" || name == ".git" {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "SKILL.md" || name.to_lowercase().ends_with(".md") {
                    out.push(path.to_string_lossy().to_string());
                }
            }
        }
    }
}

/// Extract plain text (and note images) from an ACP prompt's content blocks,
/// matching pi-acp's `promptToPiMessage`: text is concatenated, resource
/// links / embedded resources / audio become human-readable context markers,
/// and images are passed through as pi content blocks.
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
            acp::ContentBlock::ResourceLink(r) => {
                text_parts.push(format!("\n[Context] {}", r.uri));
            }
            acp::ContentBlock::Resource(r) => match &r.resource {
                acp::EmbeddedResourceResource::TextResourceContents(t) => {
                    let mime = t
                        .mime_type
                        .clone()
                        .unwrap_or_else(|| "text/plain".to_string());
                    text_parts.push(format!(
                        "\n[Embedded Context] {} ({mime})\n{}",
                        t.uri, t.text
                    ));
                }
                acp::EmbeddedResourceResource::BlobResourceContents(b) => {
                    let mime = b
                        .mime_type
                        .clone()
                        .unwrap_or_else(|| "application/octet-stream".to_string());
                    // Exact base64-decoded byte length (matching pi-acp's
                    // `Buffer.byteLength(b.blob, 'base64')`).
                    let bytes = base64_decode_len(&b.blob);
                    text_parts.push(format!(
                        "\n[Embedded Context] {} ({mime}, {bytes} bytes)",
                        b.uri
                    ));
                }
                _ => {}
            },
            acp::ContentBlock::Audio(a) => {
                // Not supported by pi; provide a marker so we don't silently
                // drop context (matching pi-acp).
                let bytes = base64_decode_len(&a.data);
                text_parts.push(format!(
                    "\n[Audio] ({}, {bytes} bytes) not supported by pi-acp",
                    a.mime_type
                ));
            }
            _ => {}
        }
    }
    (text_parts.join("\n"), images)
}

/// Exact byte length of a base64 string (matching pi-acp's
/// `Buffer.byteLength(data, 'base64')`). Invalid input falls back to the
/// length estimate.
fn base64_decode_len(data: &str) -> usize {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map(|bytes| bytes.len())
        .unwrap_or_else(|_| data.len() * 3 / 4)
}

/// Resolve a `provider/model` string (or a bare model id, resolved via the
/// builtin model list) to a pi `Model`, matching pi-acp's `setSessionModel`.
async fn resolve_model(
    value: &str,
) -> Result<pi_agent_core::pi_ai_types::Model, acp::Error> {
    let models = crate::core::model_registry::ModelRegistry::builtin_models_list();
    let registry = crate::core::model_registry::ModelRegistry::new(models);
    let (provider, model_id) = match value.split_once('/') {
        Some((p, m)) => (p.to_string(), m.to_string()),
        // Bare model id: resolve the provider via the model list (matching
        // pi-acp's fallback).
        None => {
            let models = registry.get_models();
            let found = models
                .iter()
                .find(|m| m.id == value)
                .ok_or_else(|| invalid_params_err("unknown model"))?;
            (found.provider.clone(), found.id.clone())
        }
    };
    registry
        .find(&provider, &model_id)
        .ok_or_else(|| invalid_params_err("unknown model"))
}

/// Whether a string is a valid thinking level (matching pi-acp's
/// `isThinkingLevel`).
fn is_thinking_level(x: &str) -> bool {
    matches!(x, "off" | "minimal" | "low" | "medium" | "high" | "xhigh")
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
        // Text is still collected.
        let prompt = vec![acp::ContentBlock::Text(acp::TextContent::new("hi"))];
        let (text, images) = extract_prompt(&prompt);
        assert_eq!(text, "hi");
        assert!(images.is_empty());
    }

    /// Resource links, embedded resources and audio become human-readable
    /// context markers instead of being silently dropped (matching pi-acp's
    /// `promptToPiMessage`).
    #[test]
    fn extract_prompt_handles_resources_and_audio() {
        let prompt = vec![
            acp::ContentBlock::ResourceLink(acp::ResourceLink::new(
                "a.md".to_string(),
                "file:///a.md".to_string(),
            )),
            acp::ContentBlock::Resource(acp::EmbeddedResource::new(
                acp::EmbeddedResourceResource::TextResourceContents(
                    acp::TextResourceContents::new("body".to_string(), "file:///b.md".to_string()),
                ),
            )),
            acp::ContentBlock::Audio(acp::AudioContent::new("AAAA".to_string(), "audio/mp3".to_string())),
        ];
        let (text, images) = extract_prompt(&prompt);
        assert!(text.contains("[Context] file:///a.md"), "got: {text}");
        assert!(
            text.contains("[Embedded Context] file:///b.md (text/plain)\nbody"),
            "got: {text}"
        );
        assert!(
            text.contains("[Audio] (audio/mp3, 3 bytes) not supported"),
            "got: {text}"
        );
        assert!(images.is_empty());
    }
}
