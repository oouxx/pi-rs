//! ACP session management.
//!
//! Each ACP session is backed by a pi `AgentSession` owned by a dedicated
//! actor task (`SessionTask`). The task owns the command mailbox and the
//! session itself (via `Arc`); LLM prompt turns run as separate tasks on the
//! same local executor, so the actor stays responsive to every command while
//! a turn is in flight. `session/cancel` therefore works without deadlocking
//! on a shared lock: it signals the session's abort and lets the turn task
//! settle, reporting `cancelled` as the stop reason.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::core::agent_session::AgentSession;
use crate::core::sdk::{create_agent_session, CreateAgentSessionOptions};

use super::slash_commands::{
    ResolvedCommand, load_slash_commands, resolve_command, substitute_args,
};
use super::translate::EventTranslator;

/// MCP connection handle. A no-op (unit) type when the `mcp` feature is off,
/// so session/registry code stays feature-agnostic.
#[cfg(feature = "mcp")]
pub use crate::core::mcp::McpConnection;
#[cfg(not(feature = "mcp"))]
pub type McpConnection = ();

/// Connect to the given MCP servers and build pi tool definitions for their
/// tools. Returns `(tool_definitions, connections)`: the definitions go into
/// the session's `custom_tools`, and the connections are kept alive for the
/// session's lifetime.
#[cfg(feature = "mcp")]
async fn connect_mcp_servers(
    mcp_servers: &[acp::McpServer],
) -> Result<(Vec<crate::core::extensions::ToolDefinition>, Vec<McpConnection>), String> {
    let mut connections = Vec::new();
    for config in mcp_servers {
        match crate::core::mcp::McpConnection::connect(config).await {
            Ok(conn) => connections.push(conn),
            Err(e) => eprintln!("[pi] ACP: MCP server connect failed: {e}"),
        }
    }
    let mut tool_defs = Vec::new();
    for conn in &connections {
        tool_defs.extend(conn.tool_definitions());
    }
    Ok((tool_defs, connections))
}

#[cfg(not(feature = "mcp"))]
async fn connect_mcp_servers(
    _mcp_servers: &[acp::McpServer],
) -> Result<(Vec<crate::core::extensions::ToolDefinition>, Vec<McpConnection>), String> {
    Ok((Vec::new(), Vec::new()))
}

/// A prompt queued while another prompt was running (matching pi-acp's
/// turn queue: prompts arriving mid-turn are queued and started in order
/// after the current turn settles, instead of being rejected).
struct QueuedPrompt {
    text: String,
    images: Vec<pi_agent_core::pi_ai_types::ContentBlock>,
    reply: oneshot::Sender<Result<acp::PromptResponse, String>>,
}

/// A command sent to a session task.
#[allow(clippy::large_enum_variant)]
pub enum SessionCommand {
    Prompt {
        text: String,
        images: Vec<pi_agent_core::pi_ai_types::ContentBlock>,
        reply: oneshot::Sender<Result<acp::PromptResponse, String>>,
    },
    Cancel,
    SetModel {
        model: pi_agent_core::pi_ai_types::Model,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetThinkingLevel {
        level: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    GetConfigOptions {
        reply: oneshot::Sender<Vec<acp::SessionConfigOption>>,
    },
    /// Build the ACP `available_commands_update` list (pi's get_commands +
    /// file-based + builtin, matching pi-acp's `mergeCommands`).
    GetCommands {
        reply: oneshot::Sender<Vec<acp::AvailableCommand>>,
    },
    /// Set the startup-info block to emit as the first chunk of the next prompt.
    SetStartupInfo { text: String },
    Shutdown,
}

/// Handle to a running session task.
#[derive(Clone)]
pub struct SessionHandle {
    cmd_tx: mpsc::UnboundedSender<SessionCommand>,
    cwd: String,
}

impl SessionHandle {
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Send a command to the session task.
    pub fn send(&self, cmd: SessionCommand) -> Result<(), String> {
        self.cmd_tx.send(cmd).map_err(|_| "session task closed".to_string())
    }
}

/// On-disk record for an ACP session, stored in the ACP session map so that
/// `session/load` can resume a session across process restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    /// Path to the pi session JSONL file that holds this session's messages.
    pub session_file: String,
    /// Working directory of the session.
    pub cwd: String,
    /// Optional display name (not currently set by ACP clients).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Registry of ACP sessions.
///
/// Holds the in-memory `SessionHandle`s for running session tasks, plus an
/// on-disk map (`{sessions_dir}/acp/session-map.json`) that records every
/// session created by any ACP process. This lets `session/load` and
/// `session/list` work across process restarts.
pub struct SessionRegistry {
    sessions: HashMap<acp::SessionId, SessionHandle>,
    /// Storage root (defaults to `get_sessions_dir()`; overridable for tests).
    base_dir: PathBuf,
    map_path: PathBuf,
    persisted: HashMap<acp::SessionId, PersistedSession>,
    /// 是否注册内置 Rust 扩展（goal/subagent/web_search），对应 CLI
    /// `--no-extensions` 语义。默认 false（测试与无扩展场景）。
    enable_extensions: bool,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    /// Create a registry storing ACP sessions under `{sessions_dir}/acp`.
    pub fn new() -> Self {
        Self::with_base_dir(crate::config::get_sessions_dir())
    }

    /// Create a registry rooted at `base_dir` (used by tests to avoid writing
    /// to the real agent directory). ACP sessions/map are stored under
    /// `{base_dir}/acp`.
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        let map_path = base_dir.join("acp").join("session-map.json");
        let persisted = load_map(&map_path);
        Self {
            sessions: HashMap::new(),
            base_dir,
            map_path,
            persisted,
            enable_extensions: false,
        }
    }

    /// Create a new pi `AgentSession` for `cwd`, spawn its task, persist it to
    /// disk under its own JSONL file, and register it under a fresh ACP ID.
    pub async fn create(
        &mut self,
        cwd: &str,
        mcp_servers: &[acp::McpServer],
        notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
        spawn: &Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)>,
        enable_extensions: bool,
    ) -> Result<acp::SessionId, String> {
        let session_id = acp::SessionId::new(Uuid::new_v4().to_string());
        // Persist each ACP session as its own JSONL file under the ACP dir.
        let acp_dir = self.base_dir.join("acp");
        let session_file = acp_dir.join(format!("{}.jsonl", session_id.0));
        let session_file_str = session_file.to_string_lossy().to_string();
        let session_dir_str = acp_dir.to_string_lossy().to_string();

        // Pre-create the (empty) session file so it exists even before the
        // first prompt flushes a message. `set_session_file` will then see an
        // existing-empty file and write the session header immediately.
        let _ = std::fs::create_dir_all(&acp_dir);
        let _ = std::fs::File::create(&session_file);

        let ui = Some(Self::create_acp_ui_context(notif_tx.clone(), session_id.clone()));
        let (session, mcp_connections) = self
            .build_session(
                cwd,
                Some(&session_file_str),
                &session_dir_str,
                mcp_servers,
                enable_extensions,
                ui,
            )
            .await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let task = SessionTask {
            session: Arc::new(session),
            cmd_rx,
            notif_tx,
            session_id: session_id.clone(),
            mcp_connections,
            file_commands: load_slash_commands(cwd),
            startup_info: None,
            startup_info_sent: false,
            replay_history: false,
            prompt_queue: std::collections::VecDeque::new(),
            turn_done_rx: None,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            spawn: spawn.clone(),
        };
        let fut: futures::future::LocalBoxFuture<'static, ()> = Box::pin(task.run());
        spawn(fut);
        self.sessions.insert(
            session_id.clone(),
            SessionHandle {
                cmd_tx,
                cwd: cwd.to_string(),
            },
        );
        self.persisted.insert(
            session_id.clone(),
            PersistedSession {
                session_file: session_file_str,
                cwd: cwd.to_string(),
                name: None,
            },
        );
        self.save_map();
        Ok(session_id)
    }

    /// Load a previously created session by ACP ID, resuming its persisted
    /// messages. Works for sessions created by a previous ACP process as long
    /// as their on-disk session file still exists. If the session is already
    /// running in memory, it is torn down and rebuilt so history is replayed
    /// and commands re-advertised (matching pi-acp's `loadSession`, which
    /// closes the existing session — some clients call `session/load` when
    /// restoring from history).
    pub async fn load(
        &mut self,
        session_id: &acp::SessionId,
        mcp_servers: &[acp::McpServer],
        notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
        spawn: &Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)>,
        cwd_override: Option<&str>,
    ) -> Result<(), String> {
        // Tear down an already-active session (without deleting its file) so
        // we can start fresh and replay history.
        if let Some(handle) = self.sessions.remove(session_id) {
            let _ = handle.send(SessionCommand::Shutdown);
        }
        let persisted = self
            .persisted
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("session not found: {session_id}"))?;
        if !Path::new(&persisted.session_file).exists() {
            return Err(format!(
                "session file missing for {session_id}: {}",
                persisted.session_file
            ));
        }
        let session_dir = Path::new(&persisted.session_file)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| self.base_dir.join("acp").to_string_lossy().to_string());

        // The client's cwd overrides the recorded one (matching pi-acp's
        // `opts?.cwd ?? stored.cwd`).
        let cwd = cwd_override.unwrap_or(&persisted.cwd).to_string();
        let ui = Some(Self::create_acp_ui_context(notif_tx.clone(), session_id.clone()));
        let (session, mcp_connections) = self
            .build_session(
                &cwd,
                Some(&persisted.session_file),
                &session_dir,
                mcp_servers,
                self.enable_extensions,
                ui,
            )
            .await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let task = SessionTask {
            session: Arc::new(session),
            cmd_rx,
            notif_tx,
            session_id: session_id.clone(),
            mcp_connections,
            file_commands: load_slash_commands(&cwd),
            startup_info: None,
            startup_info_sent: false,
            replay_history: true,
            prompt_queue: std::collections::VecDeque::new(),
            turn_done_rx: None,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            spawn: spawn.clone(),
        };
        let fut: futures::future::LocalBoxFuture<'static, ()> = Box::pin(task.run());
        spawn(fut);
        self.sessions.insert(
            session_id.clone(),
            SessionHandle {
                cmd_tx,
                cwd: cwd.clone(),
            },
        );
        Ok(())
    }

    /// 构造 ACP 模式的扩展 UI 上下文（统一 `extension_ui_request` 协议）。
    ///
    /// ACP 规范没有扩展 UI 的标准通道，使用 `SessionInfoUpdate._meta`
    /// 保留元数据传输协议行——标准客户端（Zed 等）按规范忽略 `_meta`，
    /// 了解协议的客户端（未来 GUI / 测试）可消费。dialog 类方法
    /// （confirm/select/input）因 ACP 无客户端回复通道而返回默认值。
    fn create_acp_ui_context(
        notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
        session_id: acp::SessionId,
    ) -> crate::core::extensions::ExtensionUIContext {
        let send_ui = {
            let notif_tx = notif_tx.clone();
            move |request: serde_json::Value| {
                let mut meta = serde_json::Map::new();
                meta.insert("extensionUiRequest".to_string(), request);
                let update = acp::SessionInfoUpdate::new().meta(meta);
                let notif = acp::SessionNotification::new(
                    session_id.clone(),
                    acp::SessionUpdate::SessionInfoUpdate(update),
                );
                let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
                let _ = notif_tx.send((notif, ack_tx));
            }
        };
        let noop = crate::core::extensions::ExtensionUIContext::noop();
        crate::core::extensions::ExtensionUIContext {
            notify: {
                let send_ui = send_ui.clone();
                std::sync::Arc::new(move |msg: &str, level: &serde_json::Value| {
                    let notify_type = level
                        .get("level")
                        .and_then(|v| v.as_str())
                        .unwrap_or("info");
                    send_ui(serde_json::json!({
                        "type": "extension_ui_request",
                        "method": "notify",
                        "message": msg,
                        "notifyType": notify_type,
                    }));
                })
            },
            set_status: {
                let send_ui = send_ui.clone();
                std::sync::Arc::new(move |key: &str, text: &str| {
                    send_ui(serde_json::json!({
                        "type": "extension_ui_request",
                        "method": "setStatus",
                        "statusKey": key,
                        "statusText": text,
                    }));
                })
            },
            set_widget: {
                let send_ui = send_ui.clone();
                std::sync::Arc::new(
                    move |key: &str, lines: Option<&[String]>, _opts: Option<&serde_json::Value>| {
                        send_ui(serde_json::json!({
                            "type": "extension_ui_request",
                            "method": "setWidget",
                            "widgetKey": key,
                            "widgetLines": lines,
                        }));
                    },
                )
            },
            set_title: {
                let send_ui = send_ui.clone();
                std::sync::Arc::new(move |title: &str| {
                    send_ui(serde_json::json!({
                        "type": "extension_ui_request",
                        "method": "setTitle",
                        "title": title,
                    }));
                })
            },
            set_editor_text: {
                let send_ui = send_ui.clone();
                std::sync::Arc::new(move |text: &str| {
                    send_ui(serde_json::json!({
                        "type": "extension_ui_request",
                        "method": "set_editor_text",
                        "text": text,
                    }));
                })
            },
            // dialog 类：ACP 无客户端回复通道 → 默认返回（confirm=false、
            // select/input=None）。与原版 ACP（pi-acp 无扩展 UI）一致。
            confirm: noop.confirm,
            select: noop.select,
            input: noop.input,
        }
    }

    /// Build a pi `AgentSession` for the given cwd, optionally backed by an
    /// existing or new session file. When `session_file` points to an existing
    /// file the session's messages are restored. MCP servers are connected and
    /// their tools injected as `custom_tools`.
    async fn build_session(
        &self,
        cwd: &str,
        session_file: Option<&str>,
        session_dir: &str,
        mcp_servers: &[acp::McpServer],
        enable_extensions: bool,
        ui: Option<crate::core::extensions::ExtensionUIContext>,
    ) -> Result<(AgentSession, Vec<McpConnection>), String> {
        let (mcp_tools, mcp_connections) = connect_mcp_servers(mcp_servers).await?;
        let custom_tools = if mcp_tools.is_empty() {
            None
        } else {
            Some(mcp_tools)
        };
        let agent_dir = crate::config::get_agent_dir();
        let (cwd_owned, session_file_owned, session_dir_owned) = (
            cwd.to_string(),
            session_file.map(|s| s.to_string()),
            session_dir.to_string(),
        );
        let build_options = move |model: Option<pi_agent_core::pi_ai_types::Model>| {
            CreateAgentSessionOptions {
                cwd: cwd_owned.clone(),
                agent_dir: Some(agent_dir.to_string_lossy().to_string()),
                model,
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
                extension_paths: vec![],
                enable_extensions: true,
                persist_session: true,
                session_file: session_file_owned.clone(),
                fork_from: None,
                session_dir: Some(session_dir_owned.clone()),
                extension_registry: crate::core::extensions::builtin_extension_registry(
                    enable_extensions,
                ),
                cli_provider: None,
                cli_model: None,
                auth_storage: None,
                model_registry: None,
                resource_loader: None,
                session_manager: None,
                settings_manager: None,
                session_start_event: None,
        ui_context: ui,
                custom_tools: custom_tools.clone(),
                extension_flags: None,
            }
        };
        let (session, _result) = create_agent_session(build_options(None))
            .await
            .map_err(|e| e.to_string())?;
        Ok((session, mcp_connections))
    }

    /// Look up a session handle by ID.
    pub fn get(&self, session_id: &acp::SessionId) -> Option<&SessionHandle> {
        self.sessions.get(session_id)
    }

    /// The storage root this registry persists ACP sessions under
    /// (`{base_dir}/acp/`). `session/list` scans it (plus pi's own session
    /// subdirectories) so the list is scoped to this agent's storage.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Delete a session: stop its task, remove it from the registry and the
    /// on-disk map, and delete its session file (idempotent — deleting a
    /// session that does not exist succeeds, matching ACP `session/delete`
    /// semantics).
    pub fn delete(&mut self, session_id: &acp::SessionId) {
        if let Some(handle) = self.sessions.remove(session_id) {
            let _ = handle.send(SessionCommand::Shutdown);
        }
        if let Some(persisted) = self.persisted.remove(session_id) {
            let _ = std::fs::remove_file(&persisted.session_file);
            self.save_map();
        }
    }

    /// List sessions: running in-memory ones plus persisted-but-not-loaded ones
    /// (so `session/list` survives restarts).
    pub fn list(&self) -> Vec<acp::SessionInfo> {
        let mut infos: Vec<acp::SessionInfo> = self
            .sessions
            .iter()
            .map(|(id, h)| acp::SessionInfo::new(id.clone(), PathBuf::from(&h.cwd)))
            .collect();
        for (id, p) in &self.persisted {
            if !self.sessions.contains_key(id) {
                infos.push(acp::SessionInfo::new(id.clone(), PathBuf::from(&p.cwd)));
            }
        }
        infos
    }

    /// Write the ACP session map to disk.
    fn save_map(&self) {
        if let Some(parent) = self.map_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&self.persisted) {
            Ok(content) => {
                if let Err(e) = std::fs::write(&self.map_path, content) {
                    eprintln!("[pi] ACP: failed to write session map {}: {e}", self.map_path.display());
                }
            }
            Err(e) => {
                eprintln!("[pi] ACP: failed to serialize session map: {e}");
            }
        }
    }
}

/// Load the ACP session map from disk, returning an empty map if it does not
/// exist. A corrupt/malformed map is treated as empty (with a warning) rather
/// than failing the whole ACP connection.
fn load_map(path: &Path) -> HashMap<acp::SessionId, PersistedSession> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    match serde_json::from_str(&content) {
        Ok(map) => map,
        Err(e) => {
            eprintln!(
                "[pi] ACP: ignoring corrupt session map {}: {e}",
                path.display()
            );
            HashMap::new()
        }
    }
}

/// The task that owns one pi `AgentSession` and drives ACP commands.
/// Result of the session task's next event: a command from the mailbox, or
/// the current turn settling (so the next queued prompt can start).
#[allow(clippy::large_enum_variant)]
enum NextEvent {
    Command(Option<SessionCommand>),
    TurnDone(Result<(), String>),
}

struct SessionTask {
    session: Arc<AgentSession>,
    cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
    session_id: acp::SessionId,
    /// Connected MCP servers; kept alive for the session's lifetime so their
    /// tool-execute closures (which capture a peer handle) stay valid.
    #[allow(dead_code)]
    mcp_connections: Vec<McpConnection>,
    /// File-based slash commands loaded for this session's cwd.
    file_commands: Vec<super::slash_commands::FileSlashCommand>,
    /// Startup-info block to emit as the first chunk of the next prompt.
    startup_info: Option<String>,
    /// Whether the startup info has already been emitted.
    startup_info_sent: bool,
    /// Whether to replay the persisted conversation history on startup
    /// (set for `session/load`).
    replay_history: bool,
    /// Prompts queued while another prompt was running (pi-acp turn queue).
    prompt_queue: std::collections::VecDeque<QueuedPrompt>,
    /// Set while an LLM turn is in flight; the receiver fires with the turn's
    /// outcome when it settles, letting the actor start the next queued prompt.
    turn_done_rx: Option<mpsc::Receiver<Result<(), String>>>,
    /// Set by `session/cancel`; the turn task reads it after the run settles
    /// to report `cancelled` instead of `end_turn`. Reset per turn.
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Local-future spawner (the same executor the session task itself runs
    /// on) used to spawn turn tasks.
    spawn: Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)>,
}

impl SessionTask {
    /// The actor loop: process commands, and when the in-flight turn settles,
    /// start the next queued prompt (matching pi-acp's turn queue). Commands
    /// are never rejected for "busy" — only prompts queue, everything else is
    /// handled immediately (matching pi-acp, which applies e.g. set_model
    /// while a turn runs).
    async fn run(mut self) {
        // Replay persisted conversation history (session/load) before
        // processing any commands, so the client sees the full context.
        if self.replay_history {
            self.replay_history().await;
        }
        loop {
            match self.next_event().await {
                NextEvent::Command(Some(SessionCommand::Prompt { text, images, reply })) => {
                    self.on_prompt(text, images, reply).await;
                }
                NextEvent::Command(Some(SessionCommand::Cancel)) => {
                    self.on_cancel().await;
                }
                NextEvent::Command(Some(SessionCommand::SetModel { model, reply })) => {
                    let result = self.session.set_model(model).await;
                    let _ = reply.send(result);
                }
                NextEvent::Command(Some(SessionCommand::SetThinkingLevel { level, reply })) => {
                    self.session.set_thinking_level(&level).await;
                    let _ = reply.send(Ok(()));
                }
                NextEvent::Command(Some(SessionCommand::GetConfigOptions { reply })) => {
                    let options = self.config_options().await;
                    let _ = reply.send(options);
                }
                NextEvent::Command(Some(SessionCommand::GetCommands { reply })) => {
                    let commands = self.available_commands().await;
                    let _ = reply.send(commands);
                }
                NextEvent::Command(Some(SessionCommand::SetStartupInfo { text })) => {
                    self.startup_info = Some(text);
                }
                NextEvent::Command(Some(SessionCommand::Shutdown)) | NextEvent::Command(None) => {
                    // Stop any in-flight turn before the session task exits so
                    // it doesn't keep running against a torn-down session.
                    if self.turn_done_rx.is_some() {
                        let session = Arc::clone(&self.session);
                        let fut: futures::future::LocalBoxFuture<'static, ()> =
                            Box::pin(async move {
                                session.abort().await;
                            });
                        (self.spawn)(fut);
                    }
                    break;
                }
                NextEvent::TurnDone(outcome) => {
                    // The turn settled. Publish the queue depth, then either
                    // start the next queued prompt or (on failure) reject the
                    // rest of the queue — pi may be unhealthy, so we don't
                    // auto-proceed after a failure (matching pi-acp).
                    self.turn_done_rx = None;
                    let _ = self.emit_queue_depth(self.prompt_queue.len(), false).await;
                    if outcome.is_err() {
                        while let Some(queued) = self.prompt_queue.pop_front() {
                            let _ = queued.reply.send(Err(
                                "session is busy processing a prompt".to_string(),
                            ));
                        }
                    } else if let Some(queued) = self.prompt_queue.pop_front() {
                        self.start_prompt_turn(queued);
                    }
                }
            }
        }
    }

    /// Wait for the next event: a command from the mailbox, or (while a turn
    /// is in flight) the turn settling.
    async fn next_event(&mut self) -> NextEvent {
        match &mut self.turn_done_rx {
            Some(rx) => tokio::select! {
                cmd = self.cmd_rx.recv() => NextEvent::Command(cmd),
                done = rx.recv() => match done {
                    Some(outcome) => NextEvent::TurnDone(outcome),
                    // The turn task died without reporting — treat as a
                    // failed turn so the queue is rejected rather than stuck.
                    None => NextEvent::TurnDone(Err("turn task ended unexpectedly".to_string())),
                },
            },
            None => NextEvent::Command(self.cmd_rx.recv().await),
        }
    }

    /// Replay the persisted conversation as ACP notifications (mirrors
    /// pi-acp's `session/load` history replay in `agent.ts`).
    async fn replay_history(&mut self) {
        use pi_agent_core::types::AgentMessage;
        let messages = self.session.get_messages().await;
        // Track bash commands from assistant tool calls so replayed bash
        // terminals show the command (matching the live path's title) instead
        // of the bare tool name.
        let mut bash_commands: HashMap<String, String> = HashMap::new();
        for msg in messages {
            match msg {
                AgentMessage::User { content, .. } => {
                    let text = content_text(&content);
                    if !text.is_empty() {
                        let _ = self.emit_user_chunk(&text).await;
                    }
                }
                AgentMessage::Assistant { content, .. } => {
                    // Record bash commands from tool calls in this message;
                    // the matching ToolResult arrives later in the stream.
                    for block in &content {
                        if let pi_agent_core::pi_ai_types::ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } = block
                        {
                            if super::translate::is_bash_tool(name) {
                                if let Some(cmd) = super::translate::bash_command(arguments) {
                                    bash_commands.insert(id.clone(), cmd);
                                }
                            }
                        }
                    }
                    let text = content_text(&content);
                    if !text.is_empty() {
                        let _ = self.emit_text(&text).await;
                    }
                }
                AgentMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    is_error,
                    ..
                } => {
                    let text = content_text(&content);
                    let status = if is_error {
                        acp::ToolCallStatus::Failed
                    } else {
                        acp::ToolCallStatus::Completed
                    };
                    let is_bash = super::translate::is_bash_tool(&tool_name);
                    // Synthetic tool call so the client renders historic tool usage.
                    // Bash results are rendered as display-only terminals
                    // (matching pi-acp's `session/load` replay), with the
                    // command as title when we saw the tool call stream in.
                    let title = if is_bash {
                        bash_commands
                            .get(&tool_call_id)
                            .cloned()
                            .unwrap_or_else(|| tool_name.clone())
                    } else {
                        tool_name.clone()
                    };
                    let mut tc = acp::ToolCall::new(tool_call_id.clone(), title)
                        .kind(super::translate::tool_kind(&tool_name))
                        .status(status);
                    if is_bash {
                        tc = tc.content(vec![acp::ToolCallContent::Terminal(acp::Terminal::new(
                            acp::TerminalId::new(tool_call_id.clone()),
                        ))]);
                        tc = tc.meta(super::translate::bash_terminal_info_meta(
                            &tool_call_id,
                            self.session.get_cwd(),
                        ));
                    }
                    let notif = acp::SessionNotification::new(
                        self.session_id.clone(),
                        acp::SessionUpdate::ToolCall(tc),
                    );
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if self.notif_tx.send((notif, ack_tx)).is_err() {
                        return;
                    }
                    let _ = ack_rx.await;
                    if !text.is_empty() {
                        let mut update = acp::ToolCallUpdate::new(
                            tool_call_id,
                            acp::ToolCallUpdateFields::new().status(status),
                        );
                        if is_bash {
                            // Stream the captured output + close the terminal
                            // with the exit code. Both keys go into ONE meta
                            // (matching pi-acp's spread) — `meta()` replaces
                            // rather than merges, so two chained calls would
                            // drop the output.
                            let id = update.tool_call_id.0.clone();
                            let mut meta = super::translate::bash_terminal_output_meta(
                                &id,
                                &text,
                            );
                            for (k, v) in super::translate::bash_terminal_exit_meta(
                                &id,
                                if is_error { 1 } else { 0 },
                            ) {
                                meta.insert(k, v);
                            }
                            update = update.meta(meta);
                        } else {
                            update.fields.content = Some(vec![acp::ToolCallContent::Content(
                                acp::Content::new(acp::ContentBlock::Text(acp::TextContent::new(
                                    text,
                                ))),
                            )]);
                        }
                        let notif = acp::SessionNotification::new(
                            self.session_id.clone(),
                            acp::SessionUpdate::ToolCallUpdate(update),
                        );
                        let (ack_tx, ack_rx) = oneshot::channel();
                        if self.notif_tx.send((notif, ack_tx)).is_err() {
                            return;
                        }
                        let _ = ack_rx.await;
                    }
                }
                _ => {}
            }
        }
    }

    /// Handle a `session/prompt` command: emit startup info once, then either
    /// start the prompt immediately or queue it (matching pi-acp's turn
    /// queue: prompts arriving mid-turn are queued and started in order after
    /// the current turn settles, instead of being rejected).
    async fn on_prompt(
        &mut self,
        text: String,
        images: Vec<pi_agent_core::pi_ai_types::ContentBlock>,
        reply: oneshot::Sender<Result<acp::PromptResponse, String>>,
    ) {
        // Emit the startup-info block once, as the first chunk of the first
        // prompt (mirrors pi-acp's `sendStartupInfoIfPending`).
        if !self.startup_info_sent {
            self.startup_info_sent = true;
            if let Some(info) = self.startup_info.take() {
                if self.emit_text(&info).await.is_err() {
                    let _ = reply.send(Err("client disconnected".to_string()));
                    return;
                }
            }
        }
        let prompt = QueuedPrompt { text, images, reply };
        if self.turn_done_rx.is_some() {
            // Queue the prompt; it runs after the current turn settles
            // (matching pi-acp's turn queue). Notify the client.
            self.prompt_queue.push_back(prompt);
            let _ = self
                .emit_text(&format!(
                    "Queued message (position {}).",
                    self.prompt_queue.len()
                ))
                .await;
            let _ = self.emit_queue_depth(self.prompt_queue.len(), true).await;
        } else {
            self.start_prompt(prompt).await;
        }
    }

    /// Start processing one prompt: resolve slash commands (built-ins execute
    /// inline and their result is emitted as a message chunk; file commands
    /// expand with `$1`/`$2`/`$@` substitution), otherwise spawn an LLM turn
    /// task. Slash commands only apply to plain-text prompts (no images);
    /// leading whitespace is trimmed before detection (matching pi-acp's
    /// `message.trimStart().startsWith('/')`).
    async fn start_prompt(&mut self, prompt: QueuedPrompt) {
        if prompt.images.is_empty() {
            let trimmed = prompt.text.trim_start().to_string();
            if let Some((cmd, args)) = resolve_command(&trimmed, &self.file_commands) {
                match cmd {
                    ResolvedCommand::Builtin(name) => {
                        let result = self.run_builtin_command(&name, &args).await;
                        let response = match result {
                            Ok(()) => Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)),
                            Err(e) => Err(e),
                        };
                        let _ = prompt.reply.send(response);
                        return;
                    }
                    ResolvedCommand::File(f) => {
                        let expanded = substitute_args(&f.content, &args);
                        self.start_prompt_turn(QueuedPrompt {
                            text: expanded,
                            images: prompt.images,
                            reply: prompt.reply,
                        });
                        return;
                    }
                }
            }
        }
        self.start_prompt_turn(prompt);
    }

    /// Spawn the LLM turn as a task on the session's executor. The task
    /// streams pi events as ACP notifications, replies with the stop reason,
    /// and reports the outcome on a fresh `turn_done_rx` channel so the actor
    /// can start the next queued prompt.
    fn start_prompt_turn(&mut self, prompt: QueuedPrompt) {
        let (turn_done_tx, turn_done_rx) = mpsc::channel::<Result<(), String>>(1);
        self.turn_done_rx = Some(turn_done_rx);
        // A cancel flag from a previous turn must not leak into this one.
        self.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);
        let fut = run_prompt_turn(
            Arc::clone(&self.session),
            self.session_id.clone(),
            self.notif_tx.clone(),
            Arc::clone(&self.cancelled),
            turn_done_tx,
            prompt,
        );
        let fut: futures::future::LocalBoxFuture<'static, ()> = Box::pin(fut);
        (self.spawn)(fut);
    }

    /// `session/cancel`: stop the current turn without blocking the command
    /// loop, resolve queued prompts as cancelled (matching pi-acp), and let
    /// the turn task report `cancelled` once the run settles.
    async fn on_cancel(&mut self) {
        if self.turn_done_rx.is_none() {
            // Nothing is running — no-op.
            return;
        }
        self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        // Signal the run to stop. `abort()` waits for the agent to become
        // idle, so it runs as a separate task; the turn task observes the
        // abort, settles, and replies `cancelled`.
        let session = Arc::clone(&self.session);
        let fut: futures::future::LocalBoxFuture<'static, ()> = Box::pin(async move {
            session.abort().await;
        });
        (self.spawn)(fut);
        // Cancel clears the queued prompts (matching pi-acp: cancel resolves
        // queued turns as cancelled).
        while let Some(queued) = self.prompt_queue.pop_front() {
            let _ = queued
                .reply
                .send(Ok(acp::PromptResponse::new(acp::StopReason::Cancelled)));
        }
        // Notify the client the queue was cleared (matching pi-acp).
        let _ = self.emit_text("Cleared queued prompts.").await;
        let _ = self.emit_queue_depth(0, true).await;
    }

    /// Emit a `session_info_update` carrying the pi-acp queue-depth metadata
    /// (`_meta.piAcp.queueDepth` / `running`).
    async fn emit_queue_depth(&self, depth: usize, running: bool) -> Result<(), String> {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "piAcp".to_string(),
            serde_json::json!({
                "queueDepth": depth,
                "running": running,
            }),
        );
        let notif = acp::SessionNotification::new(
            self.session_id.clone(),
            acp::SessionUpdate::SessionInfoUpdate(
                acp::SessionInfoUpdate::new().meta(meta),
            ),
        );
        let (ack_tx, ack_rx) = oneshot::channel();
        self.notif_tx
            .send((notif, ack_tx))
            .map_err(|_| "client disconnected".to_string())?;
        ack_rx.await.map_err(|_| "client disconnected".to_string())
    }

    /// Emit a plain-text message chunk notification to the client.
    async fn emit_text(&self, text: &str) -> Result<(), String> {
        let notif = acp::SessionNotification::new(
            self.session_id.clone(),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                acp::ContentBlock::Text(acp::TextContent::new(text.to_string())),
            )),
        );
        let (ack_tx, ack_rx) = oneshot::channel();
        self.notif_tx
            .send((notif, ack_tx))
            .map_err(|_| "client disconnected".to_string())?;
        ack_rx.await.map_err(|_| "client disconnected".to_string())
    }

    /// Emit a user message chunk notification to the client.
    async fn emit_user_chunk(&self, text: &str) -> Result<(), String> {
        let notif = acp::SessionNotification::new(
            self.session_id.clone(),
            acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(
                acp::ContentBlock::Text(acp::TextContent::new(text.to_string())),
            )),
        );
        let (ack_tx, ack_rx) = oneshot::channel();
        self.notif_tx
            .send((notif, ack_tx))
            .map_err(|_| "client disconnected".to_string())?;
        ack_rx.await.map_err(|_| "client disconnected".to_string())
    }

    /// Execute a built-in slash command and emit its result as a message
    /// chunk (mirrors pi-acp's built-in command handling in `agent.ts`).
    async fn run_builtin_command(&self, name: &str, args: &[String]) -> Result<(), String> {
        use pi_agent_core::types::QueueMode;

        match name {
            "compact" => {
                let custom = if args.is_empty() {
                    None
                } else {
                    Some(args.join(" "))
                };
                match self.session.compact(custom.as_deref()).await {
                    Ok(result) => {
                        let mut lines = vec!["Compaction completed.".to_string()];
                        if custom.is_some() {
                            lines.push("(custom instructions applied)".to_string());
                        }
                        if result.tokens_before > 0 {
                            lines.push(format!("Tokens before: {}", result.tokens_before));
                        }
                        if !result.summary.is_empty() {
                            lines.push(String::new());
                            lines.push(result.summary);
                        }
                        self.emit_text(&lines.join("\n")).await
                    }
                    Err(e) => self.emit_text(&format!("Compaction failed: {e}")).await,
                }
            }
            "autocompact" => {
                let current = self.session.get_compaction_settings().compact_on_threshold;
                let next = match args.first().map(|s| s.as_str()) {
                    Some("on") => true,
                    Some("off") => false,
                    Some("toggle") | None => !current,
                    Some(other) => {
                        return self
                            .emit_text(&format!("Usage: /autocompact on|off|toggle (got: {other})"))
                            .await;
                    }
                };
                let mut settings = self.session.get_compaction_settings();
                settings.compact_on_threshold = next;
                self.session.set_compaction_settings(settings);
                self.emit_text(&format!("Auto-compaction: {}", if next { "on" } else { "off" }))
                    .await
            }
            "export" => {
                // Export into the session cwd with a stable name, matching
                // pi-acp's `pi-session-{id}.html` in the session cwd, and emit
                // a resource link so clients render a clickable file.
                let safe_id = self
                    .session_id
                    .0
                    .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_', "_");
                let output_path = std::path::Path::new(self.session.get_cwd())
                    .join(format!("pi-session-{safe_id}.html"));
                match self.session.export_html_to_file(Some(&output_path.to_string_lossy())) {
                    Ok(path) => {
                        let uri = format!("file://{path}");
                        let _ = self.emit_text("Session exported: ").await;
                        let notif = acp::SessionNotification::new(
                            self.session_id.clone(),
                            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                                acp::ContentBlock::ResourceLink(
                                    acp::ResourceLink::new(
                                        format!("pi-session-{safe_id}.html"),
                                        uri,
                                    )
                                    .mime_type("text/html")
                                    .title("Session exported"),
                                ),
                            )),
                        );
                        let (ack_tx, ack_rx) = oneshot::channel();
                        self.notif_tx.send((notif, ack_tx)).map_err(|_| "client disconnected".to_string())?;
                        ack_rx.await.map_err(|_| "client disconnected".to_string())
                    }
                    Err(e) => self.emit_text(&format!("Export failed: {e}")).await,
                }
            }
            "session" => {
                let stats = self.session.get_session_stats();
                let mut lines = Vec::new();
                if !stats.session_id.is_empty() {
                    lines.push(format!("Session: {}", stats.session_id));
                }
                if let Some(file) = &stats.session_file {
                    lines.push(format!("Session file: {file}"));
                }
                lines.push(format!("Messages: {}", stats.total_messages));
                lines.push(format!("Cost: {:.4}", stats.cost));
                let t = &stats.tokens;
                let mut parts = Vec::new();
                if t.input > 0 {
                    parts.push(format!("in {}", t.input));
                }
                if t.output > 0 {
                    parts.push(format!("out {}", t.output));
                }
                if t.cache_read > 0 {
                    parts.push(format!("cache read {}", t.cache_read));
                }
                if t.cache_write > 0 {
                    parts.push(format!("cache write {}", t.cache_write));
                }
                if t.total > 0 {
                    parts.push(format!("total {}", t.total));
                }
                if !parts.is_empty() {
                    lines.push(format!("Tokens: {}", parts.join(", ")));
                }
                self.emit_text(&lines.join("\n")).await
            }
            "name" => {
                let name = args.join(" ").trim().to_string();
                if name.is_empty() {
                    return self.emit_text("Usage: /name <name>").await;
                }
                self.session.set_session_name(&name);
                // Keep the client's session title in sync (matching pi-acp's
                // `session_info_update` after `/name`).
                let notif = acp::SessionNotification::new(
                    self.session_id.clone(),
                    acp::SessionUpdate::SessionInfoUpdate(
                        acp::SessionInfoUpdate::new().title(name.clone()),
                    ),
                );
                let (ack_tx, ack_rx) = oneshot::channel();
                self.notif_tx.send((notif, ack_tx)).map_err(|_| "client disconnected".to_string())?;
                let _ = ack_rx.await;
                self.emit_text(&format!("Session name set to: {name}")).await
            }
            "queue" => {
                let mode = match args.first().map(|s| s.as_str()) {
                    Some("all") => QueueMode::All,
                    Some("one-at-a-time") => QueueMode::OneAtATime,
                    Some(other) => {
                        return self
                            .emit_text(&format!("Usage: /queue all|one-at-a-time (got: {other})"))
                            .await;
                    }
                    None => {
                        return self.emit_text("Usage: /queue all|one-at-a-time").await;
                    }
                };
                self.session.set_steering_mode(mode).await;
                self.session.set_follow_up_mode(mode).await;
                self.emit_text(&format!("Queue mode: {}", if mode == QueueMode::All { "all" } else { "one-at-a-time" }))
                    .await
            }
            "steering" => {
                if let Some(mode) = args.first() {
                    let mode = match mode.as_str() {
                        "all" => QueueMode::All,
                        "one-at-a-time" => QueueMode::OneAtATime,
                        other => {
                            return self
                                .emit_text(&format!("Usage: /steering all|one-at-a-time (got: {other})"))
                                .await;
                        }
                    };
                    self.session.set_steering_mode(mode).await;
                }
                let current = self.session.steering_mode().await;
                let label = if current == QueueMode::All { "all" } else { "one-at-a-time" };
                self.emit_text(&format!("Steering mode: {label}")).await
            }
            "follow-up" => {
                if let Some(mode) = args.first() {
                    let mode = match mode.as_str() {
                        "all" => QueueMode::All,
                        "one-at-a-time" => QueueMode::OneAtATime,
                        other => {
                            return self
                                .emit_text(&format!("Usage: /follow-up all|one-at-a-time (got: {other})"))
                                .await;
                        }
                    };
                    self.session.set_follow_up_mode(mode).await;
                }
                let current = self.session.follow_up_mode().await;
                let label = if current == QueueMode::All { "all" } else { "one-at-a-time" };
                self.emit_text(&format!("Follow-up mode: {label}")).await
            }
            "changelog" => {
                // Best-effort: the Rust port has no bundled changelog; point at
                // the version instead (pi-acp prints the npm changelog).
                self.emit_text(&format!(
                    "pi-coding-agent v{} (changelog not bundled in the Rust port)",
                    crate::config::VERSION
                ))
                .await
            }
            other => self.emit_text(&format!("Unknown command: /{other}")).await,
        }
    }

    /// Build the ACP session config options (model + thinking level).
    async fn config_options(&self) -> Vec<acp::SessionConfigOption> {
        let mut options = Vec::new();

        // Model selector — list all available models, current one selected.
        let model = self.session.get_model().await;
        // A session without a model (empty id) must not advertise a bogus
        // `provider/` current value — Zed can't resolve it and may report
        // "no language model configured". Use an empty current value so the
        // client shows "no model selected" and the user can pick one.
        let current_model_id = if model.id.is_empty() {
            String::new()
        } else {
            format!("{}/{}", model.provider, model.id)
        };
        let model_options: Vec<acp::SessionConfigSelectOption> = self
            .session
            .get_model_registry()
            .get_models()
            .iter()
            .map(|m| {
                let id = format!("{}/{}", m.provider, m.id);
                // Label includes the provider prefix (matching pi-acp's
                // `name: \`${provider}/${name}\``) so the client's dropdown
                // shows which provider each model belongs to.
                acp::SessionConfigSelectOption::new(
                    id,
                    format!("{}/{}", m.provider, m.name),
                )
            })
            .collect();
        let model_option = acp::SessionConfigOption::new(
            "model",
            "Model",
            acp::SessionConfigKind::Select(acp::SessionConfigSelect::new(
                current_model_id,
                model_options,
            )),
        )
        .category(acp::SessionConfigOptionCategory::Model)
        .description("Select the model for this session");
        options.push(model_option);

        // Thinking level selector. The config id is `thought_level` (matching
        // pi-acp's `THOUGHT_LEVEL_CONFIG_ID`); `set_session_config_option`
        // accepts both it and the legacy `thinking_level` spelling.
        let level = self.session.get_thinking_level().await;
        let levels = self.session.get_available_thinking_levels().await;
        let level_options: Vec<acp::SessionConfigSelectOption> = levels
            .iter()
            .map(|l| {
                // Label matches pi-acp's `name: \`Thinking: ${id}\``.
                acp::SessionConfigSelectOption::new(
                    l.to_string(),
                    format!("Thinking: {l}"),
                )
            })
            .collect();
        let level_option = acp::SessionConfigOption::new(
            "thought_level",
            "Thinking",
            acp::SessionConfigKind::Select(acp::SessionConfigSelect::new(
                level.to_string(),
                level_options,
            )),
        )
        .category(acp::SessionConfigOptionCategory::ThoughtLevel)
        .description("Set the reasoning effort for this session");
        options.push(level_option);

        options
    }

    /// Build the ACP `available_commands_update` list: pi's own discoverable
    /// commands (prompt templates + skills; extension commands are excluded,
    /// matching pi-acp's `includeExtensionCommands: false`), then file-based
    /// commands, then built-ins — de-duped by name, first wins (matching
    /// pi-acp's `mergeCommands`).
    async fn available_commands(&self) -> Vec<acp::AvailableCommand> {
        let mut commands: Vec<acp::AvailableCommand> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        let mut push = |name: String, description: String| {
            if seen.insert(name.clone()) {
                commands.push(acp::AvailableCommand::new(name, description));
            }
        };

        // pi's get_commands: prompt templates + skills (no extension commands).
        for info in self.session.get_commands_info() {
            if matches!(
                info.source,
                crate::core::slash_commands::SlashCommandSource::Extension
            ) {
                continue;
            }
            let description = info
                .description
                .unwrap_or_else(|| format!("({})", info.source_info.source));
            push(info.name, description);
        }

        // File-based prompt templates (user + project).
        for c in &self.file_commands {
            push(c.name.clone(), c.description.clone());
        }

        // Built-ins.
        for c in super::slash_commands::builtin_available_commands() {
            push(c.name.clone(), c.description.clone());
        }

        commands
    }
}

/// Spawned by the session task: run one LLM turn against the shared session,
/// reply with the stop reason, then report the outcome so the actor can
/// start the next queued prompt.
async fn run_prompt_turn(
    session: Arc<AgentSession>,
    session_id: acp::SessionId,
    notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    turn_done: mpsc::Sender<Result<(), String>>,
    prompt: QueuedPrompt,
) {
    let QueuedPrompt { text, images, reply } = prompt;
    let response =
        run_single_prompt(&session, &session_id, &notif_tx, &cancelled, &text, images).await;
    let outcome = response.as_ref().map(|_| ()).map_err(|e| e.clone());
    let _ = reply.send(response);
    let _ = turn_done.send(outcome).await;
}

/// Run one prompt turn: stream pi events as ACP notifications, then report
/// the stop reason. `cancelled` is set by the session task on
/// `session/cancel`; the response then reports `cancelled` instead of
/// `end_turn` (matching pi-acp).
async fn run_single_prompt(
    session: &AgentSession,
    session_id: &acp::SessionId,
    notif_tx: &mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
    cancelled: &std::sync::atomic::AtomicBool,
    text: &str,
    images: Vec<pi_agent_core::pi_ai_types::ContentBlock>,
) -> Result<acp::PromptResponse, String> {
    let (event_tx, mut event_rx) =
        mpsc::unbounded_channel::<crate::core::agent_session::AgentSessionEvent>();
    let listener: Arc<
        dyn Fn(crate::core::agent_session::AgentSessionEvent) + Send + Sync,
    > = Arc::new(move |event| {
        let _ = event_tx.send(event);
    });
    let handle = session.subscribe_session_events(listener);

    let prompt_options = crate::core::agent_session::PromptOptions {
        expand_prompt_templates: None,
        images: (!images.is_empty()).then_some(images),
        streaming_behavior: None,
        source: Some("acp".to_string()),
        preflight_result: None,
    };
    let prompt_fut = session.prompt(text, Some(prompt_options));
    tokio::pin!(prompt_fut);

    // Track whether the agent actually ran a turn (mirrors the RPC mode's
    // AgentEnd check: prompt() returns Ok even when the run fails to start).
    let mut saw_agent_end = false;
    // Acks of notifications forwarded while the turn ran. We don't await
    // them inline (that would delay reacting to the prompt future); they are
    // flushed when the prompt future completes (matching pi-acp's
    // fire-and-forget emit + flushEmits on agent_settled).
    let mut pending_acks: Vec<oneshot::Receiver<()>> = Vec::new();

    // Per-turn translator: tool-call state (snapshots / statuses) is keyed by
    // tool-call id, which is unique per call, so nothing carries across turns.
    let mut translator = EventTranslator::new(session.get_cwd());

    let outcome: Result<(), String> = loop {
        tokio::select! {
            result = &mut prompt_fut => {
                // Drain any events still queued (e.g. AgentEnd) so the
                // saw_agent_end check below is accurate: prompt() may
                // return before the select! loop has consumed the final
                // events.
                let mut disconnected = false;
                while let Ok(event) = event_rx.try_recv() {
                    if matches!(event, crate::core::agent_session::AgentSessionEvent::AgentEnd { .. }) {
                        saw_agent_end = true;
                    }
                    if let Some(notif) = translator.translate(session_id, &event) {
                        let (ack_tx, ack_rx) = oneshot::channel();
                        if notif_tx.send((notif, ack_tx)).is_err() {
                            disconnected = true;
                            break;
                        }
                        pending_acks.push(ack_rx);
                    }
                }
                // Flush: wait for all forwarded notifications to be
                // delivered before replying to the prompt.
                for ack in pending_acks.drain(..) {
                    if ack.await.is_err() {
                        disconnected = true;
                        break;
                    }
                }
                if disconnected {
                    break Err("client disconnected".to_string());
                }
                break result;
            }
            Some(event) = event_rx.recv() => {
                if matches!(event, crate::core::agent_session::AgentSessionEvent::AgentEnd { .. }) {
                    saw_agent_end = true;
                }
                if let Some(notif) = translator.translate(session_id, &event) {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if notif_tx.send((notif, ack_tx)).is_err() {
                        break Err("client disconnected".to_string());
                    }
                    pending_acks.push(ack_rx);
                }
            }
        }
    };

    handle.unsubscribe();
    let cancel_requested = cancelled.load(std::sync::atomic::Ordering::SeqCst);
    let response = if saw_agent_end {
        let reason = if cancel_requested {
            acp::StopReason::Cancelled
        } else {
            acp::StopReason::EndTurn
        };
        Ok(acp::PromptResponse::new(reason))
    } else {
        Err("agent run failed to start or completed without AgentEnd".to_string())
    };
    outcome.and(response)
}

/// Extract the plain text from a message's content blocks (text + thinking).
fn content_text(content: &[pi_agent_core::pi_ai_types::ContentBlock]) -> String {
    let mut parts = Vec::new();
    for block in content {
        match block {
            pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } => {
                parts.push(text.clone());
            }
            pi_agent_core::pi_ai_types::ContentBlock::Thinking { thinking, .. } => {
                parts.push(thinking.clone());
            }
            _ => {}
        }
    }
    parts.join("\n")
}

/// A pi session discovered on disk (mirrors pi-acp's `PiSessionListItem`).
#[derive(Debug, Clone)]
pub struct PiSessionItem {
    pub session_id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

/// Recursively scan `dir` for pi session JSONL files and extract the session
/// id / cwd / title / updatedAt, mirroring pi-acp's `listPiSessions`:
/// - header line (`{"type":"session","id":…,"cwd":…}`) gives id + cwd
/// - the last `session_info` entry with a name gives the title
/// - the last `message` entry's timestamp gives updatedAt
/// - fallbacks: file mtime for updatedAt, first user message for title
///
/// Scoped to the agent's storage root so `session/list` is isolated per agent
/// (tests use a tempdir; production uses `{sessions_dir}` which also contains
/// regular CLI sessions in per-cwd subdirectories).
pub fn scan_pi_sessions(dir: &std::path::Path) -> Vec<PiSessionItem> {
    let mut files = Vec::new();
    walk_jsonl_files(dir, &mut files);

    let mut items = Vec::new();
    for file in files {
        let Some(item) = scan_session_file(&file) else {
            continue;
        };
        items.push(item);
    }

    // Sort most recent first (matching pi-acp).
    items.sort_by(|a, b| {
        let aa = a.updated_at.as_deref().unwrap_or("");
        let bb = b.updated_at.as_deref().unwrap_or("");
        bb.cmp(aa)
    });
    items
}

fn walk_jsonl_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl_files(&path, out);
        } else if path.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("jsonl")
        {
            out.push(path);
        }
    }
}

fn scan_session_file(path: &std::path::Path) -> Option<PiSessionItem> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();

    // Header line: {"type":"session","id":…,"cwd":…}
    let header: serde_json::Value = serde_json::from_str(lines.next()?.trim()).ok()?;
    if header.get("type").and_then(|t| t.as_str()) != Some("session") {
        return None;
    }
    let session_id = header.get("id")?.as_str()?.to_string();
    let cwd = header.get("cwd")?.as_str()?.to_string();

    let mut title: Option<String> = None;
    let mut updated_at: Option<String> = None;
    let mut first_user_message: Option<String> = None;

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("session_info") => {
                if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                    if !name.trim().is_empty() {
                        title = Some(name.trim().to_string());
                    }
                }
            }
            Some("message") => {
                if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                    updated_at = Some(ts.to_string());
                }
                if first_user_message.is_none() {
                    let role = v
                        .get("message")
                        .and_then(|m| m.get("role"))
                        .and_then(|r| r.as_str());
                    if role == Some("user") {
                        first_user_message = extract_first_text(&v);
                    }
                }
            }
            _ => {}
        }
    }

    // Fallback: file mtime for updatedAt.
    if updated_at.is_none() {
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                let dt: chrono::DateTime<chrono::Utc> = modified.into();
                updated_at = Some(dt.to_rfc3339());
            }
        }
    }
    // Fallback: first user message for title.
    if title.is_none() {
        title = first_user_message.map(|t| {
            let truncated: String = t.chars().take(80).collect();
            truncated
        });
    }

    Some(PiSessionItem {
        session_id,
        cwd,
        title,
        updated_at,
    })
}

/// Extract the first text block from a message entry's `message.content`.
fn extract_first_text(entry: &serde_json::Value) -> Option<String> {
    let content = entry.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let arr = content.as_array()?;
    for block in arr {
        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                return Some(t.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// A no-op spawner for registry-level tests (the session task itself is
    /// not exercised here — only persistence/registry bookkeeping is).
    fn noop_spawn() -> Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)> {
        Arc::new(|_fut| {})
    }

    fn notif_tx() -> mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)> {
        let (tx, _rx) = mpsc::unbounded_channel();
        tx
    }

    /// `session/new` persists a session file + map entry, and a fresh registry
    /// created from the same directory can `session/load` it by ID — i.e.
    /// sessions survive a process restart.
    #[tokio::test]
    async fn load_session_survives_registry_recreation() {
        let base = tempfile::tempdir().expect("tempdir");
        let base_path = base.path().to_path_buf();
        let spawn = noop_spawn();

        // "Process 1": create a session; it must be persisted to disk.
        let sid;
        {
            let mut reg = SessionRegistry::with_base_dir(base_path.clone());
            sid = reg.create("/tmp", &[], notif_tx(), &spawn, false).await.expect("create");
            let acp_dir = base_path.join("acp");
            assert!(
                acp_dir.join(format!("{}.jsonl", sid.0)).exists(),
                "session file must be persisted for {sid}"
            );
            assert!(
                acp_dir.join("session-map.json").exists(),
                "session map must be persisted"
            );
            // list() includes the just-created session.
            assert_eq!(reg.list().len(), 1);
        }

        // "Process 2": a brand-new registry from the same directory.
        let mut reg = SessionRegistry::with_base_dir(base_path.clone());
        assert!(
            reg.get(&sid).is_none(),
            "fresh registry must not know the session in memory"
        );
        // It is still listed (persisted-but-not-loaded).
        assert_eq!(reg.list().len(), 1);
        assert_eq!(reg.list()[0].session_id, sid);

        // Loading by ID resumes it.
        reg.load(&sid, &[], notif_tx(), &spawn, None).await.expect("load cross-process");
        assert!(reg.get(&sid).is_some(), "session should be loaded after restart");

        // Loading again is a no-op (already running in memory).
        reg.load(&sid, &[], notif_tx(), &spawn, None).await.expect("re-load is a no-op");
    }

    /// `session/load` of an unknown ID (never created, or from an unrelated
    /// directory) must fail cleanly rather than panic.
    #[tokio::test]
    async fn load_unknown_session_errors() {
        let base = tempfile::tempdir().expect("tempdir");
        let spawn = noop_spawn();
        let mut reg = SessionRegistry::with_base_dir(base.path().to_path_buf());
        let unknown = acp::SessionId::new(Uuid::new_v4().to_string());
        let err = reg.load(&unknown, &[], notif_tx(), &spawn, None).await.unwrap_err();
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    /// `session/close` (delete) removes the session from the registry, deletes
    /// its on-disk file, and is idempotent for unknown IDs.
    #[tokio::test]
    async fn delete_removes_session_and_file() {
        let base = tempfile::tempdir().expect("tempdir");
        let spawn = noop_spawn();
        let mut reg = SessionRegistry::with_base_dir(base.path().to_path_buf());
        let sid = reg.create("/tmp", &[], notif_tx(), &spawn, false).await.expect("create");
        let session_file = base.path().join("acp").join(format!("{}.jsonl", sid.0));
        assert!(session_file.exists(), "session file must exist");

        reg.delete(&sid);
        assert!(reg.get(&sid).is_none(), "session must be removed from registry");
        assert!(!session_file.exists(), "session file must be deleted");
        assert_eq!(reg.list().len(), 0, "session must not be listed");

        // Deleting again (unknown ID) is a no-op, not an error.
        reg.delete(&sid);
    }

    /// `scan_pi_sessions` parses session id / cwd / title / updatedAt from
    /// JSONL files (matching pi-acp's `listPiSessions`): header line for
    /// id+cwd, last `session_info` name for title, last message timestamp for
    /// updatedAt, and falls back to the first user message for the title.
    #[test]
    fn scan_pi_sessions_parses_header_title_and_updated_at() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("proj");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("s1.jsonl");
        std::fs::write(
            &file,
            concat!(
                "{\"type\":\"session\",\"version\":3,\"id\":\"s1\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"cwd\":\"/proj\"}\n",
                "{\"type\":\"message\",\"id\":\"m1\",\"timestamp\":\"2026-01-01T00:01:00Z\",\"message\":{\"role\":\"user\",\"content\":\"hello world\"}}\n",
                "{\"type\":\"session_info\",\"id\":\"i1\",\"timestamp\":\"2026-01-01T00:02:00Z\",\"name\":\"My Session\"}\n",
            ),
        )
        .unwrap();

        let items = scan_pi_sessions(dir.path());
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.session_id, "s1");
        assert_eq!(item.cwd, "/proj");
        assert_eq!(item.title.as_deref(), Some("My Session"));
        assert_eq!(item.updated_at.as_deref(), Some("2026-01-01T00:01:00Z"));
    }

    /// A session file without a `session_info` name falls back to the first
    /// user message as the title (matching pi-acp's `pickFallbackTitleFromHead`).
    #[test]
    fn scan_pi_sessions_falls_back_to_first_user_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("s2.jsonl");
        std::fs::write(
            &file,
            concat!(
                "{\"type\":\"session\",\"version\":3,\"id\":\"s2\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"cwd\":\"/proj\"}\n",
                "{\"type\":\"message\",\"id\":\"m1\",\"timestamp\":\"2026-01-01T00:01:00Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Fix the bug please\"}]}}\n",
            ),
        )
        .unwrap();

        let items = scan_pi_sessions(dir.path());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title.as_deref(), Some("Fix the bug please"));
    }

    // =====================================================================
    // SessionTask actor tests (real AgentSession + fake stream_fn)
    // =====================================================================

    fn fake_model() -> pi_agent_core::pi_ai_types::Model {
        pi_agent_core::pi_ai_types::Model {
            id: "test-model".into(),
            name: "Test Model".into(),
            api: "test-api".into(),
            provider: "test-provider".into(),
            base_url: "https://test.invalid".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: pi_agent_core::pi_ai_types::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: vec![],
            },
            context_window: 128_000,
            max_tokens: 8192,
            headers: None,
            compat: None,
        }
    }

    /// A model registry whose `test-provider` has an API key, so the auth
    /// check in `prompt()` passes without env vars.
    fn fake_model_registry() -> crate::core::model_registry::ModelRegistry {
        let registry = crate::core::model_registry::ModelRegistry::new(vec![fake_model()]);
        registry.register_provider(
            "test-provider",
            crate::core::model_registry::ProviderConfig {
                name: Some("Test Provider".into()),
                base_url: None,
                api_key: Some("test-key".into()),
                api: Some("test-api".into()),
                headers: None,
                auth_header: None,
            },
        );
        registry
    }

    /// A minimal `AssistantMessageEvent::Done` carrying the model identity
    /// (the agent loop finalizes the turn with it).
    fn fake_done_event(
        model: &pi_agent_core::pi_ai_types::Model,
    ) -> pi_agent_core::pi_ai_types::AssistantMessageEvent {
        pi_agent_core::pi_ai_types::AssistantMessageEvent::Done {
            reason: pi_agent_core::pi_ai_types::StopReason::Stop,
            message: pi_agent_core::pi_ai_types::AssistantMessage {
                content: vec![],
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: pi_agent_core::pi_ai_types::Usage::default(),
                stop_reason: pi_agent_core::pi_ai_types::StopReason::Stop,
                error_message: None,
                raw_stop_reason: None,
                timestamp: 0,
            },
        }
    }

    /// The event a real provider stream emits when the abort signal fires
    /// (matching pi-ai: `stop_reason = "aborted"` so downstream retry and
    /// compaction checks skip the turn).
    fn fake_aborted_event(
        model: &pi_agent_core::pi_ai_types::Model,
    ) -> pi_agent_core::pi_ai_types::AssistantMessageEvent {
        pi_agent_core::pi_ai_types::AssistantMessageEvent::Error {
            reason: pi_agent_core::pi_ai_types::StopReason::Aborted,
            error: pi_agent_core::pi_ai_types::AssistantMessage {
                content: vec![],
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: pi_agent_core::pi_ai_types::Usage::default(),
                stop_reason: pi_agent_core::pi_ai_types::StopReason::Aborted,
                error_message: Some("Request was aborted".to_string()),
                raw_stop_reason: None,
                timestamp: 0,
            },
        }
    }

    /// Create an `AgentSession` wired to the given fake stream_fn (no real
    /// LLM, no extensions, no persistence).
    async fn test_agent_session(
        stream_fn: pi_agent_core::types::StreamFn,
        base_dir: &std::path::Path,
    ) -> AgentSession {
        let (session, _result) = create_agent_session(CreateAgentSessionOptions {
            cwd: "/tmp".to_string(),
            agent_dir: Some(base_dir.to_string_lossy().to_string()),
            model: Some(fake_model()),
            thinking_level: None,
            scoped_models: None,
            no_tools: Some(crate::core::sdk::NoToolsMode::All),
            tools: None,
            exclude_tools: None,
            custom_prompt: None,
            append_system_prompt: None,
            session_name: None,
            stream_fn: Some(stream_fn),
            convert_to_llm: None,
            custom_tools: None,
            extension_flags: None,
            extension_paths: vec![],
            enable_extensions: false,
            extension_registry: None,
            cli_provider: None,
            cli_model: None,
            persist_session: false,
            session_file: None,
            fork_from: None,
            session_dir: None,
            auth_storage: None,
            model_registry: Some(fake_model_registry()),
            resource_loader: None,
            session_manager: None,
            settings_manager: None,
            session_start_event: None,
            ui_context: None,
        })
        .await
        .expect("create test agent session");
        session
    }

    /// Wire a `SessionTask` on a `LocalSet` (the same executor shape the ACP
    /// mode uses) and return the command sender plus a collector of the
    /// notifications the task emitted (acked immediately, like a client).
    fn spawn_session_task(
        local: &tokio::task::LocalSet,
        spawn: &Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)>,
        session: AgentSession,
        replay_history: bool,
    ) -> (
        mpsc::UnboundedSender<SessionCommand>,
        Arc<std::sync::Mutex<Vec<acp::SessionNotification>>>,
    ) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (notif_tx, mut notif_rx) =
            mpsc::unbounded_channel::<(acp::SessionNotification, oneshot::Sender<()>)>();
        let collected = Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected2 = collected.clone();
        // Ack notifications as they arrive so the turn task's ack flush
        // doesn't block waiting for an absent client.
        local.spawn_local(async move {
            while let Some((notif, ack)) = notif_rx.recv().await {
                collected2
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(notif);
                let _ = ack.send(());
            }
        });
        let task = SessionTask {
            session: Arc::new(session),
            cmd_rx,
            notif_tx,
            session_id: acp::SessionId::new(String::from("test-session")),
            mcp_connections: vec![],
            file_commands: vec![],
            startup_info: None,
            startup_info_sent: false,
            replay_history,
            prompt_queue: std::collections::VecDeque::new(),
            turn_done_rx: None,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            spawn: spawn.clone(),
        };
        let fut: futures::future::LocalBoxFuture<'static, ()> = Box::pin(task.run());
        local.spawn_local(fut);
        (cmd_tx, collected)
    }

    fn send_prompt(
        cmd_tx: &mpsc::UnboundedSender<SessionCommand>,
        text: &str,
    ) -> oneshot::Receiver<Result<acp::PromptResponse, String>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(SessionCommand::Prompt {
                text: text.to_string(),
                images: vec![],
                reply: reply_tx,
            })
            .expect("send prompt");
        reply_rx
    }

    /// A stream_fn whose first call blocks until the abort signal fires
    /// (simulating a long-running LLM call) and whose later calls complete
    /// immediately. Returns the call counter too.
    fn blocking_then_completing_stream() -> (
        pi_agent_core::types::StreamFn,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let stream_fn: pi_agent_core::types::StreamFn = Arc::new(
            move |model: pi_agent_core::pi_ai_types::Model,
                  _ctx: pi_agent_core::pi_ai_types::Context,
                  _thinking: Option<pi_agent_core::pi_ai_types::ThinkingLevel>,
                  opts: pi_agent_core::types::StreamFnOptions| {
                let call = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let signal = opts.signal.clone();
                let model = model.clone();
                Box::pin(async move {
                    let stream = futures::stream::unfold(call, move |call| {
                        let mut signal = signal.clone();
                        let model = model.clone();
                        // Box the future so the unfold stream is Unpin.
                        Box::pin(async move {
                            if call > 0 {
                                // Later turns complete immediately.
                                return Some((fake_done_event(&model), call + 1));
                            }
                            // First turn: block until the abort signal fires,
                            // then report the turn as aborted (matching the
                            // real provider streams).
                            if let Some(rx) = &mut signal {
                                if !*rx.borrow() {
                                    let _ = Box::pin(rx.changed()).await;
                                }
                            }
                            if signal
                                .as_ref()
                                .map(|rx| *rx.borrow())
                                .unwrap_or(false)
                            {
                                return Some((fake_aborted_event(&model), call + 1));
                            }
                            None
                        })
                    });
                    Ok(Box::new(stream) as pi_agent_core::pi_ai_types::StreamResponse)
                })
            },
        );
        (stream_fn, calls)
    }

    /// A stream_fn whose first call completes after a delay and whose later
    /// calls complete immediately.
    fn delayed_completing_stream() -> (
        pi_agent_core::types::StreamFn,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let stream_fn: pi_agent_core::types::StreamFn = Arc::new(
            move |model: pi_agent_core::pi_ai_types::Model,
                  _ctx: pi_agent_core::pi_ai_types::Context,
                  _thinking: Option<pi_agent_core::pi_ai_types::ThinkingLevel>,
                  _opts: pi_agent_core::types::StreamFnOptions| {
                let call = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let delay = if call == 0 {
                    std::time::Duration::from_millis(200)
                } else {
                    std::time::Duration::ZERO
                };
                let model = model.clone();
                Box::pin(async move {
                    let stream = futures::stream::unfold(0u8, move |state| {
                        let model = model.clone();
                        // Box the future so the unfold stream is Unpin.
                        Box::pin(async move {
                            if state == 0 {
                                Box::pin(tokio::time::sleep(delay)).await;
                                return Some((fake_done_event(&model), 1));
                            }
                            None
                        })
                    });
                    Ok(Box::new(stream) as pi_agent_core::pi_ai_types::StreamResponse)
                })
            },
        );
        (stream_fn, calls)
    }

    fn chunk_text(chunk: &acp::ContentChunk) -> String {
        match &chunk.content {
            acp::ContentBlock::Text(t) => t.text.clone(),
            _ => String::new(),
        }
    }

    /// Wait until `calls` reaches the given count (the turn has actually
    /// started), yielding to the LocalSet so the session/turn tasks run.
    async fn wait_for_calls(
        calls: &Arc<std::sync::atomic::AtomicUsize>,
        expected: usize,
    ) {
        for _ in 0..200 {
            if calls.load(std::sync::atomic::Ordering::SeqCst) >= expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!(
            "timed out waiting for stream call {} (got {})",
            expected,
            calls.load(std::sync::atomic::Ordering::SeqCst)
        );
    }

    /// `session/cancel` during a running turn must (1) be non-blocking,
    /// (2) report `cancelled` for the interrupted prompt, and (3) leave the
    /// session usable — the next prompt runs a normal `end_turn` turn.
    #[tokio::test(flavor = "current_thread")]
    async fn cancel_then_prompt_again() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let spawn: Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let base = tempfile::tempdir().expect("tempdir");
                let (stream_fn, calls) = blocking_then_completing_stream();
                let session = test_agent_session(stream_fn, base.path()).await;
                let (cmd_tx, _notifs) = spawn_session_task(&local, &spawn, session, false);

                // Prompt 1: the fake stream blocks, so the turn stays in flight.
                let reply_rx = send_prompt(&cmd_tx, "hello 1");
                wait_for_calls(&calls, 1).await;

                // Cancel: the interrupted prompt must report `cancelled`.
                cmd_tx.send(SessionCommand::Cancel).unwrap();
                let resp = reply_rx
                    .await
                    .expect("cancelled prompt must reply")
                    .expect("cancelled prompt must succeed");
                assert_eq!(resp.stop_reason, acp::StopReason::Cancelled);

                // Prompt 2 after cancel: runs a normal turn.
                let reply_rx2 = send_prompt(&cmd_tx, "hello 2");
                let resp2 = reply_rx2
                    .await
                    .expect("second prompt must reply")
                    .expect("second prompt must succeed");
                assert_eq!(resp2.stop_reason, acp::StopReason::EndTurn);
                assert_eq!(
                    calls.load(std::sync::atomic::Ordering::SeqCst),
                    2,
                    "second turn must have run"
                );
            })
            .await;
    }

    /// Prompts arriving while a turn is in flight are queued and started in
    /// order after the current turn settles (matching pi-acp's turn queue) —
    /// the actor drains the queue on turn-done instead of rejecting them, and
    /// the client is told the message was queued.
    #[tokio::test(flavor = "current_thread")]
    async fn prompts_queue_behind_running_turn() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let spawn: Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let base = tempfile::tempdir().expect("tempdir");
                let (stream_fn, calls) = delayed_completing_stream();
                let session = test_agent_session(stream_fn, base.path()).await;
                let (cmd_tx, notifs) = spawn_session_task(&local, &spawn, session, false);

                // Prompt A: first stream call takes 200ms, so turn A is in
                // flight for a while.
                let reply_a = send_prompt(&cmd_tx, "prompt A");
                wait_for_calls(&calls, 1).await;

                // Prompt B while A is running: must be queued, not rejected.
                let reply_b = send_prompt(&cmd_tx, "prompt B");

                // Both turns settle in order, each reporting `end_turn`.
                let resp_a = reply_a
                    .await
                    .expect("prompt A must reply")
                    .expect("prompt A must succeed");
                assert_eq!(resp_a.stop_reason, acp::StopReason::EndTurn);
                let resp_b = reply_b
                    .await
                    .expect("prompt B must reply")
                    .expect("prompt B must succeed");
                assert_eq!(resp_b.stop_reason, acp::StopReason::EndTurn);
                assert_eq!(
                    calls.load(std::sync::atomic::Ordering::SeqCst),
                    2,
                    "queued turn must have run after the first settled"
                );

                // The client must have been told prompt B was queued.
                let queued = notifs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .any(|n| {
                        matches!(&n.update, acp::SessionUpdate::AgentMessageChunk(c)
                            if chunk_text(c).contains("Queued message"))
                    });
                assert!(queued, "client must be notified the prompt was queued");
            })
            .await;
    }

    /// Commands other than prompts are handled immediately even while a turn
    /// is in flight (the actor stays responsive) — they are not rejected with
    /// "session is busy".
    #[tokio::test(flavor = "current_thread")]
    async fn commands_handled_during_turn() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let spawn: Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let base = tempfile::tempdir().expect("tempdir");
                let (stream_fn, calls) = blocking_then_completing_stream();
                let session = test_agent_session(stream_fn, base.path()).await;
                let (cmd_tx, _notifs) = spawn_session_task(&local, &spawn, session, false);

                // Blocking turn in flight.
                let reply_rx = send_prompt(&cmd_tx, "hello");
                wait_for_calls(&calls, 1).await;

                // GetConfigOptions must return the real options (not the
                // empty fallback the old busy-rejection used).
                let (reply_tx, reply_rx_cfg) = oneshot::channel();
                cmd_tx
                    .send(SessionCommand::GetConfigOptions {
                        reply: reply_tx,
                    })
                    .unwrap();
                let options = reply_rx_cfg.await.expect("config options reply");
                assert!(
                    !options.is_empty(),
                    "config options must be served during a turn"
                );

                // SetStartupInfo is applied while a turn runs.
                cmd_tx
                    .send(SessionCommand::SetStartupInfo {
                        text: "startup".into(),
                    })
                    .unwrap();

                // The turn itself still cancels cleanly afterwards.
                cmd_tx.send(SessionCommand::Cancel).unwrap();
                let resp = reply_rx
                    .await
                    .expect("prompt must reply")
                    .expect("prompt must succeed");
                assert_eq!(resp.stop_reason, acp::StopReason::Cancelled);
            })
            .await;
    }

    /// `session/load` history replay must render bash tool results as
    /// terminals whose title is the command (extracted from the assistant
    /// tool call), not the bare tool name — matching the live path.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_history_uses_bash_command_as_title() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let spawn: Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let base = tempfile::tempdir().expect("tempdir");
                let (stream_fn, _calls) = blocking_then_completing_stream();
                let session = test_agent_session(stream_fn, base.path()).await;

                // Seed the session with a bash turn: assistant tool call
                // (carrying the command) followed by its tool result.
                use pi_agent_core::types::AgentMessage;
                session
                    .get_agent()
                    .set_initial_messages(vec![
                        AgentMessage::Assistant {
                            content: vec![pi_agent_core::pi_ai_types::ContentBlock::ToolCall {
                                id: "b1".into(),
                                name: "bash".into(),
                                arguments: serde_json::json!({"command": "ls -la"}),
                                thought_signature: None,
                            }],
                            api: "openai-completions".into(),
                            provider: "openai".into(),
                            model: "gpt-5.5".into(),
                            usage: pi_agent_core::pi_ai_types::Usage::default(),
                            stop_reason: Some(pi_agent_core::pi_ai_types::StopReason::ToolUse),
                            error_message: None,
                            timestamp: 0,
                        },
                        AgentMessage::ToolResult {
                            tool_call_id: "b1".into(),
                            tool_name: "bash".into(),
                            content: vec![pi_agent_core::pi_ai_types::ContentBlock::Text {
                                text: "total 0\n".into(),
                                text_signature: None,
                            }],
                            details: serde_json::json!({}),
                            is_error: false,
                            added_tool_names: None,
                            usage: None,
                            timestamp: 0,
                        },
                    ])
                    .await;

                let (_cmd_tx, notifs) =
                    spawn_session_task(&local, &spawn, session, true);

                // Let the replay run to completion.
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                let collected = notifs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let bash_tool_call = collected.iter().find_map(|n| match &n.update {
                    acp::SessionUpdate::ToolCall(tc)
                        if tc.tool_call_id.0.as_ref() == "b1" =>
                    {
                        Some(tc.clone())
                    }
                    _ => None,
                });
                let tc = bash_tool_call.expect("replay must emit the bash tool call");
                assert_eq!(
                    tc.title, "ls -la",
                    "replayed bash terminal must show the command, not the tool name"
                );
                assert!(
                    matches!(tc.kind, acp::ToolKind::Execute),
                    "bash must render as an execute terminal"
                );

                // The tool OUTPUT must also be restored: the replay streams
                // the captured stdout into the terminal via terminal_output.
                let out_meta = collected.iter().find_map(|n| match &n.update {
                    acp::SessionUpdate::ToolCallUpdate(tcu)
                        if tcu.tool_call_id.0.as_ref() == "b1" =>
                    {
                        tcu.meta.clone()
                    }
                    _ => None,
                });
                let meta = out_meta.expect("replay must emit a tool_call_update");
                let term_out = meta
                    .get("terminal_output")
                    .expect("bash output must stream via terminal_output meta");
                assert_eq!(
                    term_out["data"].as_str(),
                    Some("total 0\n"),
                    "replayed terminal must contain the captured output"
                );
                let term_exit = meta
                    .get("terminal_exit")
                    .expect("bash terminal must be closed with an exit code");
                assert_eq!(term_exit["exit_code"], 0);
            })
            .await;
    }

    /// `--no-extensions` 语义在 ACP 会话上的体现：`build_session` 的
    /// `enable_extensions` 决定是否注册内置 Rust 扩展工具
    /// （goal/subagent/web_search）。
    #[tokio::test]
    async fn test_build_session_extension_tools() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = SessionRegistry::with_base_dir(tmp.path().to_path_buf());

        // 启用：工具列表含全部内置扩展工具。
        let (session, _conns) = reg
            .build_session("/tmp", None, "/tmp", &[], true, None)
            .await
            .expect("build with extensions");
        let state = session.get_agent().state().await;
        let names: Vec<String> = state.tools.iter().map(|t| t.name.clone()).collect();
        for tool in ["web_search", "web_fetch", "subagent", "goal_complete", "goal_blocked", "goal_wait"] {
            assert!(names.contains(&tool.to_string()), "missing {tool}: {names:?}");
        }

        // 禁用（--no-extensions）：无任何扩展工具。
        let (session, _conns) = reg
            .build_session("/tmp", None, "/tmp", &[], false, None)
            .await
            .expect("build without extensions");
        let state = session.get_agent().state().await;
        let names: Vec<String> = state.tools.iter().map(|t| t.name.clone()).collect();
        for tool in ["web_search", "web_fetch", "subagent", "goal_complete"] {
            assert!(!names.contains(&tool.to_string()), "unexpected {tool}: {names:?}");
        }
        // 内置基础工具不受影响。
        assert!(names.contains(&"bash".to_string()));
    }
}
