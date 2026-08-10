//! ACP session management.
//!
//! Each ACP session is backed by a pi `AgentSession` owned by a dedicated
//! task (`SessionTask`). The task is the single owner of the session, which
//! lets `session/cancel` interrupt a running `session/prompt` without
//! deadlocking on a shared lock: while a prompt is in flight the task
//! `select!`s on the prompt future, the event stream, and the command
//! channel, so a `Cancel` command can call `abort()` on the same session.

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

        let (session, mcp_connections) = self
            .build_session(cwd, Some(&session_file_str), &session_dir_str, mcp_servers)
            .await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let task = SessionTask {
            session,
            cmd_rx,
            notif_tx,
            session_id: session_id.clone(),
            mcp_connections,
            translator: EventTranslator::new(cwd),
            file_commands: load_slash_commands(cwd),
            startup_info: None,
            startup_info_sent: false,
            replay_history: false,
            prompt_queue: std::collections::VecDeque::new(),
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

        let (session, mcp_connections) = self
            .build_session(&persisted.cwd, Some(&persisted.session_file), &session_dir, mcp_servers)
            .await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let task = SessionTask {
            session,
            cmd_rx,
            notif_tx,
            session_id: session_id.clone(),
            mcp_connections,
            translator: EventTranslator::new(&persisted.cwd),
            file_commands: load_slash_commands(&persisted.cwd),
            startup_info: None,
            startup_info_sent: false,
            replay_history: true,
            prompt_queue: std::collections::VecDeque::new(),
        };
        let fut: futures::future::LocalBoxFuture<'static, ()> = Box::pin(task.run());
        spawn(fut);
        self.sessions.insert(
            session_id.clone(),
            SessionHandle {
                cmd_tx,
                cwd: persisted.cwd,
            },
        );
        Ok(())
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
                extension_registry: None,
                cli_provider: None,
                cli_model: None,
                auth_storage: None,
                model_registry: None,
                resource_loader: None,
                session_manager: None,
                settings_manager: None,
                session_start_event: None,
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
struct SessionTask {
    session: AgentSession,
    cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
    session_id: acp::SessionId,
    /// Connected MCP servers; kept alive for the session's lifetime so their
    /// tool-execute closures (which capture a peer handle) stay valid.
    #[allow(dead_code)]
    mcp_connections: Vec<McpConnection>,
    /// Enriches tool-call notifications with locations / diffs / bash terminals.
    translator: EventTranslator,
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
}

impl SessionTask {
    async fn run(mut self) {
        // Replay persisted conversation history (session/load) before
        // processing any commands, so the client sees the full context.
        if self.replay_history {
            self.replay_history().await;
        }
        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                SessionCommand::Prompt { text, images, reply } => {
                    self.run_prompt(text, images, reply).await;
                }
                SessionCommand::Cancel => {
                    self.session.abort().await;
                }
                SessionCommand::SetModel { model, reply } => {
                    let result = self.session.set_model(model).await;
                    let _ = reply.send(result);
                }
                SessionCommand::SetThinkingLevel { level, reply } => {
                    self.session.set_thinking_level(&level).await;
                    let _ = reply.send(Ok(()));
                }
                SessionCommand::GetConfigOptions { reply } => {
                    let options = self.config_options().await;
                    let _ = reply.send(options);
                }
                SessionCommand::SetStartupInfo { text } => {
                    self.startup_info = Some(text);
                }
                SessionCommand::Shutdown => break,
            }
        }
    }

    /// Replay the persisted conversation as ACP notifications (mirrors
    /// pi-acp's `session/load` history replay in `agent.ts`).
    async fn replay_history(&mut self) {
        use pi_agent_core::types::AgentMessage;
        let messages = self.session.get_messages().await;
        for msg in messages {
            match msg {
                AgentMessage::User { content, .. } => {
                    let text = content_text(&content);
                    if !text.is_empty() {
                        let _ = self.emit_user_chunk(&text).await;
                    }
                }
                AgentMessage::Assistant { content, .. } => {
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
                    // Synthetic tool call so the client renders historic tool usage.
                    let tc = acp::ToolCall::new(tool_call_id.clone(), tool_name.clone())
                        .kind(super::translate::tool_kind(&tool_name))
                        .status(status);
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
                        let fields = acp::ToolCallUpdateFields::new()
                            .status(status)
                            .content(vec![acp::ToolCallContent::Content(acp::Content::new(
                                acp::ContentBlock::Text(acp::TextContent::new(text)),
                            ))]);
                        let notif = acp::SessionNotification::new(
                            self.session_id.clone(),
                            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                                tool_call_id,
                                fields,
                            )),
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

    /// Run one prompt turn: stream pi events as ACP notifications, then reply
    /// with the stop reason. `session/cancel` is handled inline so it can
    /// interrupt the running turn. `images` are passed through to pi's
    /// `prompt()` (ContentBlock::Image), matching the RPC mode.
    ///
    /// Slash commands are resolved first (matching pi-acp): built-in commands
    /// are executed directly and their result emitted as a message chunk;
    /// file-based commands are expanded (with `$1`/`$2`/`$@` substitution)
    /// and run as a normal prompt.
    async fn run_prompt(
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
        // Slash commands only apply to plain-text prompts (no images).
        if images.is_empty() {
            if let Some((cmd, args)) = resolve_command(&text, &self.file_commands) {
                match cmd {
                    ResolvedCommand::Builtin(name) => {
                        let result = self.run_builtin_command(&name, &args).await;
                        let response = match result {
                            Ok(()) => Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)),
                            Err(e) => Err(e),
                        };
                        let _ = reply.send(response);
                        return;
                    }
                    ResolvedCommand::File(f) => {
                        let expanded = substitute_args(&f.content, &args);
                        return self.run_prompt_inner(expanded, images, reply).await;
                    }
                }
            }
        }
        self.run_prompt_inner(text, images, reply).await;
    }

    /// The actual prompt loop (see `run_prompt` for slash-command handling).
    ///
    /// Runs the prompt, then any prompts queued while it was running
    /// (matching pi-acp's turn queue). A failed run stops the queue — pi may
    /// be unhealthy, so we don't auto-proceed (matching pi-acp).
    async fn run_prompt_inner(
        &mut self,
        text: String,
        images: Vec<pi_agent_core::pi_ai_types::ContentBlock>,
        reply: oneshot::Sender<Result<acp::PromptResponse, String>>,
    ) {
        let mut current = QueuedPrompt { text, images, reply };
        loop {
            let outcome = self.run_single_prompt(current.text, current.images).await;
            let _ = current.reply.send(outcome.clone());
            if outcome.is_err() {
                // Don't auto-proceed after a failure; reject anything queued.
                while let Some(queued) = self.prompt_queue.pop_front() {
                    let _ = queued
                        .reply
                        .send(Err("session is busy processing a prompt".to_string()));
                }
                break;
            }
            match self.prompt_queue.pop_front() {
                Some(next) => current = next,
                None => break,
            }
        }
    }

    /// Run one prompt turn: stream pi events as ACP notifications, then reply
    /// with the stop reason. `session/cancel` is handled inline so it can
    /// interrupt the running turn; prompts arriving mid-turn are queued
    /// (matching pi-acp) instead of rejected.
    async fn run_single_prompt(
        &mut self,
        text: String,
        images: Vec<pi_agent_core::pi_ai_types::ContentBlock>,
    ) -> Result<acp::PromptResponse, String> {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<crate::core::agent_session::AgentSessionEvent>();
        let listener: Arc<
            dyn Fn(crate::core::agent_session::AgentSessionEvent) + Send + Sync,
        > = Arc::new(move |event| {
            let _ = event_tx.send(event);
        });
        let handle = self.session.subscribe_session_events(listener);

        let prompt_options = crate::core::agent_session::PromptOptions {
            expand_prompt_templates: None,
            images: (!images.is_empty()).then_some(images),
            streaming_behavior: None,
            source: Some("acp".to_string()),
            preflight_result: None,
        };
        let prompt_fut = self.session.prompt(&text, Some(prompt_options));
        tokio::pin!(prompt_fut);

        // Track whether the agent actually ran a turn (mirrors the RPC mode's
        // AgentEnd check: prompt() returns Ok even when the run fails to start).
        let mut saw_agent_end = false;
        // Set when the client cancelled this turn; the response then reports
        // `cancelled` instead of `end_turn` (matching pi-acp).
        let mut cancel_requested = false;
        // Acks of notifications forwarded while the turn ran. We don't await
        // them inline (that would block the select loop and delay cancel);
        // they are flushed when the prompt future completes (matching
        // pi-acp's fire-and-forget emit + flushEmits on agent_settled).
        let mut pending_acks: Vec<oneshot::Receiver<()>> = Vec::new();

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
                        if let Some(notif) = self.translator.translate(&self.session_id, &event) {
                            let (ack_tx, ack_rx) = oneshot::channel();
                            if self.notif_tx.send((notif, ack_tx)).is_err() {
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
                    if let Some(notif) = self.translator.translate(&self.session_id, &event) {
                        let (ack_tx, ack_rx) = oneshot::channel();
                        if self.notif_tx.send((notif, ack_tx)).is_err() {
                            break Err("client disconnected".to_string());
                        }
                        pending_acks.push(ack_rx);
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(SessionCommand::Cancel) => {
                            cancel_requested = true;
                            self.session.abort().await;
                            // Cancel clears the queued prompts (matching
                            // pi-acp: cancel resolves queued turns as
                            // cancelled).
                            while let Some(queued) = self.prompt_queue.pop_front() {
                                let _ = queued.reply.send(Ok(
                                    acp::PromptResponse::new(acp::StopReason::Cancelled),
                                ));
                            }
                        }
                        Some(SessionCommand::Prompt { text, images, reply }) => {
                            // Queue the prompt; it runs after the current turn
                            // settles (matching pi-acp's turn queue).
                            self.prompt_queue.push_back(QueuedPrompt { text, images, reply });
                        }
                        Some(SessionCommand::Shutdown) | None => {
                            break Err("session closed".to_string());
                        }
                        Some(other) => {
                            // Session is busy — reject other commands.
                            reject_busy(other);
                        }
                    }
                }
            }
        };

        handle.unsubscribe();
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
    async fn run_builtin_command(&mut self, name: &str, args: &[String]) -> Result<(), String> {
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
                let mut settings = self.session.get_compaction_settings().clone();
                settings.compact_on_threshold = next;
                self.session.set_compaction_settings(settings);
                self.emit_text(&format!("Auto-compaction: {}", if next { "on" } else { "off" }))
                    .await
            }
            "export" => {
                match self.session.export_html_to_file(None) {
                    Ok(path) => self.emit_text(&format!("Session exported to: {path}")).await,
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
                acp::SessionConfigSelectOption::new(id, m.name.clone())
            })
            .collect();
        let model_option = acp::SessionConfigOption::new(
            "model",
            "Model",
            acp::SessionConfigKind::Select(acp::SessionConfigSelect::new(
                current_model_id,
                model_options,
            )),
        );
        options.push(model_option);

        // Thinking level selector.
        let level = self.session.get_thinking_level().await;
        let levels = self.session.get_available_thinking_levels().await;
        let level_options: Vec<acp::SessionConfigSelectOption> = levels
            .iter()
            .map(|l| acp::SessionConfigSelectOption::new(l.to_string(), l.to_string()))
            .collect();
        let level_option = acp::SessionConfigOption::new(
            "thinking_level",
            "Thinking Level",
            acp::SessionConfigKind::Select(acp::SessionConfigSelect::new(
                level.to_string(),
                level_options,
            )),
        );
        options.push(level_option);

        options
    }
}

/// Reply "session busy" to a command that arrived while a prompt was running.
fn reject_busy(cmd: SessionCommand) {
    match cmd {
        SessionCommand::Prompt { reply, .. } => {
            let _ = reply.send(Err("session is busy processing a prompt".to_string()));
        }
        SessionCommand::SetModel { reply, .. } => {
            let _ = reply.send(Err("session is busy processing a prompt".to_string()));
        }
        SessionCommand::SetThinkingLevel { reply, .. } => {
            let _ = reply.send(Err("session is busy processing a prompt".to_string()));
        }
        SessionCommand::GetConfigOptions { reply } => {
            let _ = reply.send(Vec::new());
        }
        SessionCommand::SetStartupInfo { .. } => {}
        SessionCommand::Cancel | SessionCommand::Shutdown => {}
    }
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
            sid = reg.create("/tmp", &[], notif_tx(), &spawn).await.expect("create");
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
        reg.load(&sid, &[], notif_tx(), &spawn).await.expect("load cross-process");
        assert!(reg.get(&sid).is_some(), "session should be loaded after restart");

        // Loading again is a no-op (already running in memory).
        reg.load(&sid, &[], notif_tx(), &spawn).await.expect("re-load is a no-op");
    }

    /// `session/load` of an unknown ID (never created, or from an unrelated
    /// directory) must fail cleanly rather than panic.
    #[tokio::test]
    async fn load_unknown_session_errors() {
        let base = tempfile::tempdir().expect("tempdir");
        let spawn = noop_spawn();
        let mut reg = SessionRegistry::with_base_dir(base.path().to_path_buf());
        let unknown = acp::SessionId::new(Uuid::new_v4().to_string());
        let err = reg.load(&unknown, &[], notif_tx(), &spawn).await.unwrap_err();
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    /// `session/close` (delete) removes the session from the registry, deletes
    /// its on-disk file, and is idempotent for unknown IDs.
    #[tokio::test]
    async fn delete_removes_session_and_file() {
        let base = tempfile::tempdir().expect("tempdir");
        let spawn = noop_spawn();
        let mut reg = SessionRegistry::with_base_dir(base.path().to_path_buf());
        let sid = reg.create("/tmp", &[], notif_tx(), &spawn).await.expect("create");
        let session_file = base.path().join("acp").join(format!("{}.jsonl", sid.0));
        assert!(session_file.exists(), "session file must exist");

        reg.delete(&sid);
        assert!(reg.get(&sid).is_none(), "session must be removed from registry");
        assert!(!session_file.exists(), "session file must be deleted");
        assert_eq!(reg.list().len(), 0, "session must not be listed");

        // Deleting again (unknown ID) is a no-op, not an error.
        reg.delete(&sid);
    }
}
