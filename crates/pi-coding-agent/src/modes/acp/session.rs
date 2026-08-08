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

use super::translate::translate_event;

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

        let (session, mcp_connections, js_manager) = self
            .build_session(cwd, Some(&session_file_str), &session_dir_str, mcp_servers)
            .await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let task = SessionTask {
            session,
            cmd_rx,
            notif_tx,
            session_id: session_id.clone(),
            mcp_connections,
            _js_manager: js_manager,
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
    /// running in memory, this is a no-op.
    pub async fn load(
        &mut self,
        session_id: &acp::SessionId,
        mcp_servers: &[acp::McpServer],
        notif_tx: mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>,
        spawn: &Arc<dyn Fn(futures::future::LocalBoxFuture<'static, ()>)>,
    ) -> Result<(), String> {
        if self.sessions.contains_key(session_id) {
            return Ok(());
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

        let (session, mcp_connections, js_manager) = self
            .build_session(&persisted.cwd, Some(&persisted.session_file), &session_dir, mcp_servers)
            .await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let task = SessionTask {
            session,
            cmd_rx,
            notif_tx,
            session_id: session_id.clone(),
            mcp_connections,
            _js_manager: js_manager,
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
    ) -> Result<
        (
            AgentSession,
            Vec<McpConnection>,
            Option<Box<dyn std::any::Any + Send>>,
        ),
        String,
    > {
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
            }
        };
        let (session, result) = match create_agent_session(build_options(None)).await {
            Ok(r) => r,
            Err(_) => {
                // No default model configured (fresh environment): fall back to
                // the first builtin model so the session can still be created.
                // The client can switch models later via set_session_model /
                // session config options.
                let first = crate::core::model_registry::ModelRegistry::builtin_models_list()
                    .into_iter()
                    .next();
                match first {
                    Some(m) => create_agent_session(build_options(Some(m)))
                        .await
                        .map_err(|e| e.to_string())?,
                    None => {
                        return Err(
                            "no model configured and no builtin models available".to_string()
                        );
                    }
                }
            }
        };
        // Keep the V8 extension manager alive for the session's lifetime:
        // dropping it shuts down the V8 thread, killing JS extensions.
        let js_manager = result._js_extension_manager;
        Ok((session, mcp_connections, js_manager))
    }

    /// Look up a session handle by ID.
    pub fn get(&self, session_id: &acp::SessionId) -> Option<&SessionHandle> {
        self.sessions.get(session_id)
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
    /// V8 JS-extension manager; kept alive so JS extensions work for the
    /// session's lifetime (dropping it shuts down the V8 thread).
    #[allow(dead_code)]
    _js_manager: Option<Box<dyn std::any::Any + Send>>,
}

impl SessionTask {
    async fn run(mut self) {
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
                SessionCommand::Shutdown => break,
            }
        }
    }

    /// Run one prompt turn: stream pi events as ACP notifications, then reply
    /// with the stop reason. `session/cancel` is handled inline so it can
    /// interrupt the running turn. `images` are passed through to pi's
    /// `prompt()` (ContentBlock::Image), matching the RPC mode.
    async fn run_prompt(
        &mut self,
        text: String,
        images: Vec<pi_agent_core::pi_ai_types::ContentBlock>,
        reply: oneshot::Sender<Result<acp::PromptResponse, String>>,
    ) {
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
        };
        let prompt_fut = self.session.prompt(&text, Some(prompt_options));
        tokio::pin!(prompt_fut);

        // Track whether the agent actually ran a turn (mirrors the RPC mode's
        // AgentEnd check: prompt() returns () even when the run fails to start).
        let mut saw_agent_end = false;

        let outcome: Result<(), String> = loop {
            tokio::select! {
                _ = &mut prompt_fut => {
                    break Ok(());
                }
                Some(event) = event_rx.recv() => {
                    if matches!(event, crate::core::agent_session::AgentSessionEvent::AgentEnd { .. }) {
                        saw_agent_end = true;
                    }
                    if let Some(notif) = translate_event(&self.session_id, &event) {
                        let (ack_tx, ack_rx) = oneshot::channel();
                        if self.notif_tx.send((notif, ack_tx)).is_err() {
                            break Err("client disconnected".to_string());
                        }
                        if ack_rx.await.is_err() {
                            break Err("client disconnected".to_string());
                        }
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(SessionCommand::Cancel) => {
                            self.session.abort().await;
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
            Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
        } else {
            Err("agent run failed to start or completed without AgentEnd".to_string())
        };
        let _ = reply.send(outcome.and(response));
    }

    /// Build the ACP session config options (model + thinking level).
    async fn config_options(&self) -> Vec<acp::SessionConfigOption> {
        let mut options = Vec::new();

        // Model selector — list all available models, current one selected.
        let model = self.session.get_model().await;
        let current_model_id = format!("{}/{}", model.provider, model.id);
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
        SessionCommand::Cancel | SessionCommand::Shutdown => {}
    }
}

#[cfg(test)]
mod tests {
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
}
