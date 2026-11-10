//! `/mcp` manager UI for the interactive TUI.
//!
//! Mirrors the TS `McpManagerView`: a framed, multi-level menu over the
//! transcript. The editor dock renders a [`PanelState`] (pi-tui) and this
//! module owns the state machine, live connection updates, and actions.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pi_tui::{PanelInput, PanelItem, PanelState};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::core::mcp_config::{self, LoadedServer, McpServerConfigPatch};
use crate::core::mcp_oauth::{self, McpOAuthCredentialStore, McpOAuthSettings, McpSignInPrompt};
use crate::core::mcp_runtime::{McpServerConnection, McpServerState};
use crate::core::mcp_servers::McpExposure;

use super::UiAction;

const MAX_VISIBLE_ITEMS: usize = 12;

/// Events sent from async MCP work back to the TUI event loop.
pub enum McpUiEvent {
    /// A connection attempt finished.
    Connected {
        name: String,
        state: McpServerState,
        tools: Vec<pi_mcp::Tool>,
        has_resources: bool,
        instructions: Option<String>,
        error: Option<String>,
    },
    /// An action finished with a user-facing message.
    ActionDone { name: String, message: String },
    /// The sign-in flow produced an authorization URL.
    SignInUrl { name: String, url: String },
    /// The sign-in flow finished.
    SignInDone {
        name: String,
        message: String,
        ok: bool,
    },
    /// Configuration was reloaded after an enable/disable change.
    ConfigChanged { name: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Screen {
    Servers,
    Server(String),
    Tools(String),
    Exposure(String),
    SignIn(String),
}

struct ServerUi {
    entry: LoadedServer,
    connection: Option<Arc<McpServerConnection>>,
    state: McpServerState,
    error: Option<String>,
    tools: Vec<pi_mcp::Tool>,
    has_resources: bool,
    instructions: Option<String>,
    message: Option<String>,
}

impl ServerUi {
    fn enabled(&self) -> bool {
        self.entry.config.is_enabled()
    }

    fn description(&self) -> String {
        format!(
            "{} · {} · {}",
            self.state_label(),
            self.entry.config.exposure(),
            self.entry.scope.as_str()
        )
    }

    fn state_label(&self) -> &'static str {
        if !self.enabled() {
            return "disabled";
        }
        self.state.as_str()
    }

    fn attention_rank(&self) -> u8 {
        if !self.enabled() {
            return 5;
        }
        match self.state {
            McpServerState::NeedsAuth => 0,
            McpServerState::Failed => 1,
            McpServerState::Disconnected => 2,
            McpServerState::Connecting => 3,
            McpServerState::Connected => 4,
            McpServerState::Closed => 5,
        }
    }
}

struct SignInUi {
    url: Option<String>,
    message: String,
    input: String,
    redirect: Arc<Mutex<Option<oneshot::Sender<Option<String>>>>>,
}

/// MCP manager state.
pub struct McpUiState {
    cwd: String,
    agent_dir: String,
    servers: Vec<ServerUi>,
    config_errors: Vec<String>,
    screen: Screen,
    selected: usize,
    sign_in: Option<SignInUi>,
    /// Servers whose connection this manager created (and therefore owns).
    owned: std::collections::HashSet<String>,
}

impl std::fmt::Debug for McpUiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpUiState")
            .field("servers", &self.servers.len())
            .field("screen", &self.screen)
            .finish()
    }
}

/// Result of a key press inside the manager.
pub enum McpKeyResult {
    /// The panel changed and needs a redraw.
    Redraw,
    /// Close the manager and return to the editor.
    Close,
    /// The key was not handled by the panel.
    Ignored,
}

impl McpUiState {
    /// Load configuration and start connecting enabled servers.
    pub fn new(cwd: String, agent_dir: String, ui_tx: UnboundedSender<UiAction>) -> Self {
        Self::new_with_connections(cwd, agent_dir, ui_tx, Vec::new())
    }

    /// Create the manager, reusing the session's existing connections.
    pub fn new_with_connections(
        cwd: String,
        agent_dir: String,
        ui_tx: UnboundedSender<UiAction>,
        existing: Vec<McpServerConnection>,
    ) -> Self {
        let loaded = mcp_config::load_mcp_config(&cwd, &agent_dir);
        let mut existing: std::collections::HashMap<String, McpServerConnection> = existing
            .into_iter()
            .map(|connection| (connection.name().to_string(), connection))
            .collect();
        let mut servers = Vec::new();
        let mut owned = std::collections::HashSet::new();
        for entry in loaded.servers {
            let (connection, state, error, tools, has_resources, instructions) =
                if entry.config.is_enabled() {
                    if let Some(connection) = existing.remove(&entry.name) {
                        let state = connection.state();
                        let tools = connection.tools();
                        let error = connection.error();
                        let has_resources = connection.has_resources();
                        let instructions = connection.instructions();
                        let connection = Arc::new(connection);
                        if state != McpServerState::Connected {
                            spawn_connect(Arc::clone(&connection), ui_tx.clone());
                        }
                        (
                            Some(connection),
                            state,
                            error,
                            tools,
                            has_resources,
                            instructions,
                        )
                    } else {
                        let connection = Arc::new(McpServerConnection::with_agent_dir(
                            entry.clone(),
                            &cwd,
                            &agent_dir,
                        ));
                        spawn_connect(Arc::clone(&connection), ui_tx.clone());
                        owned.insert(entry.name.clone());
                        (
                            Some(connection),
                            McpServerState::Connecting,
                            None,
                            Vec::new(),
                            false,
                            None,
                        )
                    }
                } else {
                    (None, McpServerState::Closed, None, Vec::new(), false, None)
                };
            servers.push(ServerUi {
                entry,
                connection,
                state,
                error,
                tools,
                has_resources,
                instructions,
                message: None,
            });
        }
        Self {
            cwd,
            agent_dir,
            servers,
            config_errors: loaded.errors,
            screen: Screen::Servers,
            selected: 0,
            sign_in: None,
            owned,
        }
    }

    /// Apply a `/mcp login|logout|reconnect [server]` subcommand.
    pub fn dispatch_action(&mut self, args: &str, ui_tx: &UnboundedSender<UiAction>) {
        let mut parts = args.split_whitespace();
        let action = parts.next();
        let name = parts.next();
        let Some(name) = name else {
            self.screen = Screen::Servers;
            return;
        };
        if self.find(name).is_none() {
            self.config_errors
                .push(format!("No MCP server named \"{name}\"."));
            self.screen = Screen::Servers;
            return;
        }
        match action {
            Some("login") => {
                self.start_sign_in(name, ui_tx);
                if self.sign_in.is_some() {
                    self.screen = Screen::SignIn(name.to_string());
                }
            }
            Some("logout") => {
                self.sign_out(name, ui_tx);
                self.screen = Screen::Server(name.to_string());
                self.selected = 0;
            }
            Some("reconnect") => {
                self.reconnect(name, ui_tx);
                self.screen = Screen::Server(name.to_string());
                self.selected = 0;
            }
            _ => {
                self.screen = Screen::Servers;
            }
        }
    }

    fn find(&self, name: &str) -> Option<&ServerUi> {
        self.servers.iter().find(|server| server.entry.name == name)
    }

    fn find_mut(&mut self, name: &str) -> Option<&mut ServerUi> {
        self.servers
            .iter_mut()
            .find(|server| server.entry.name == name)
    }

    fn sorted_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.servers.len()).collect();
        indices.sort_by(|a, b| {
            self.servers[*a]
                .attention_rank()
                .cmp(&self.servers[*b].attention_rank())
                .then_with(|| {
                    self.servers[*a]
                        .entry
                        .name
                        .cmp(&self.servers[*b].entry.name)
                })
        });
        indices
    }

    fn server_at_selected(&self) -> Option<&ServerUi> {
        self.sorted_indices()
            .get(self.selected)
            .and_then(|index| self.servers.get(*index))
    }

    fn item_count(&self) -> usize {
        match &self.screen {
            Screen::Servers => self.servers.len(),
            Screen::Server(name) => self
                .find(name)
                .map(server_actions)
                .map(|actions| actions.len())
                .unwrap_or(0),
            Screen::Tools(name) => self
                .find(name)
                .map(|server| server.tools.len())
                .unwrap_or(0),
            Screen::Exposure(_) => 4,
            Screen::SignIn(_) => 0,
        }
    }

    /// Build the framed panel for the current screen.
    pub fn panel(&self) -> PanelState {
        match &self.screen {
            Screen::Servers => self.servers_panel(),
            Screen::Server(name) => self.server_panel(name),
            Screen::Tools(name) => self.tools_panel(name),
            Screen::Exposure(name) => self.exposure_panel(name),
            Screen::SignIn(name) => self.sign_in_panel(name),
        }
    }

    fn servers_panel(&self) -> PanelState {
        let indices = self.sorted_indices();
        let items: Vec<PanelItem> = indices
            .iter()
            .map(|index| {
                let server = &self.servers[*index];
                PanelItem {
                    label: server.entry.name.clone(),
                    description: Some(server.description()),
                }
            })
            .collect();
        let mut error = self.config_errors.join("\n");
        if error.is_empty() {
            error = self
                .servers
                .iter()
                .filter_map(|server| {
                    server
                        .error
                        .as_ref()
                        .map(|error| format!("{}: {error}", server.entry.name))
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        PanelState {
            title: "MCP servers".to_string(),
            details: Vec::new(),
            error: (!error.is_empty()).then_some(error),
            items,
            selected: self.selected.min(self.servers.len().saturating_sub(1)),
            empty: Some(format!(
                "No MCP servers configured. Add them to {}/mcp.json or .pi-rs/mcp.json.",
                self.agent_dir
            )),
            footer: Some("enter manage • escape/ctrl+c close".to_string()),
            input: None,
        }
    }

    fn server_panel(&self, name: &str) -> PanelState {
        let Some(server) = self.find(name) else {
            return PanelState {
                title: name.to_string(),
                empty: Some("This server is no longer configured.".to_string()),
                footer: Some("escape/ctrl+c back".to_string()),
                ..Default::default()
            };
        };
        let actions = server_actions(server);
        let items = actions
            .iter()
            .map(|action| PanelItem {
                label: action.label.clone(),
                description: action.description.clone(),
            })
            .collect();
        let mut details = vec![
            describe_transport(&server.entry),
            format!(
                "{}: {}",
                server.entry.scope.as_str(),
                server.entry.source.display()
            ),
            format!("State: {}", state_summary(server)),
        ];
        if let Some(override_path) = &server.entry.override_path {
            details.push(format!("Project override: {}", override_path.display()));
        }
        let error = server.message.clone().or_else(|| server.error.clone());
        PanelState {
            title: format!("MCP server {name}"),
            details,
            error,
            items,
            selected: self.selected.min(actions.len().saturating_sub(1)),
            empty: None,
            footer: Some("enter select • escape/ctrl+c back".to_string()),
            input: None,
        }
    }

    fn tools_panel(&self, name: &str) -> PanelState {
        let Some(server) = self.find(name) else {
            return PanelState::default();
        };
        let exposure = server.entry.config.exposure();
        let overridden = !server.entry.config.tool_exposure.is_empty();
        let mut details = vec![format!("Exposure {exposure}:")];
        details.push(exposure_description(exposure).to_string());
        if overridden {
            details.push("Some tools override it with toolExposure.".to_string());
        }
        let items = server
            .tools
            .iter()
            .map(|tool| {
                let tool_exposure = server.entry.config.tool_exposure(&tool.name);
                let description = tool
                    .description
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();
                PanelItem {
                    label: tool.name.clone(),
                    description: if tool_exposure == exposure {
                        (!description.is_empty()).then_some(description)
                    } else {
                        Some(format!("[{tool_exposure}] {description}"))
                    },
                }
            })
            .collect::<Vec<_>>();
        PanelState {
            title: format!("Tools of {name}"),
            details,
            error: None,
            items,
            selected: self.selected,
            empty: Some("The server offers no tools.".to_string()),
            footer: Some("enter back • escape/ctrl+c back".to_string()),
            input: None,
        }
    }

    fn exposure_panel(&self, name: &str) -> PanelState {
        let current = self
            .find(name)
            .map(|server| server.entry.config.exposure())
            .unwrap_or(McpExposure::Codemode);
        let options = [
            McpExposure::Codemode,
            McpExposure::Deferred,
            McpExposure::Direct,
            McpExposure::Hidden,
        ];
        let items = options
            .iter()
            .map(|exposure| PanelItem {
                label: format!(
                    "{}{exposure}",
                    if *exposure == current { "✓ " } else { "  " }
                ),
                description: Some(exposure_description(*exposure).to_string()),
            })
            .collect();
        PanelState {
            title: format!("Exposure of {name}"),
            details: vec![format!("Saved to the server's mcp.json entry.")],
            error: None,
            items,
            selected: options.iter().position(|e| *e == current).unwrap_or(0),
            empty: None,
            footer: Some("enter save • escape/ctrl+c back".to_string()),
            input: None,
        }
    }

    fn sign_in_panel(&self, name: &str) -> PanelState {
        let Some(sign_in) = &self.sign_in else {
            return PanelState::default();
        };
        let mut details = Vec::new();
        if let Some(url) = &sign_in.url {
            details.push("Approve access in your browser. If it did not open, visit:".to_string());
            details.push(url.clone());
            details.push(String::new());
            details.push(
                "If the browser runs on another machine, paste the URL it was redirected to:"
                    .to_string(),
            );
        } else {
            details.push(sign_in.message.clone());
        }
        PanelState {
            title: format!("Sign in to {name}"),
            details,
            error: None,
            items: Vec::new(),
            selected: 0,
            empty: None,
            footer: Some("enter submit • escape/ctrl+c cancel".to_string()),
            input: Some(PanelInput {
                label: "redirect".to_string(),
                value: sign_in.input.clone(),
            }),
        }
    }

    /// Handle one key press.
    pub fn handle_key(&mut self, key: KeyEvent, ui_tx: &UnboundedSender<UiAction>) -> McpKeyResult {
        if matches!(self.screen, Screen::SignIn(_)) {
            return self.handle_sign_in_key(key);
        }
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                McpKeyResult::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = self.item_count();
                if self.selected + 1 < count {
                    self.selected += 1;
                }
                McpKeyResult::Redraw
            }
            KeyCode::Home => {
                self.selected = 0;
                McpKeyResult::Redraw
            }
            KeyCode::End => {
                self.selected = self.item_count().saturating_sub(1);
                McpKeyResult::Redraw
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(MAX_VISIBLE_ITEMS);
                McpKeyResult::Redraw
            }
            KeyCode::PageDown => {
                self.selected =
                    (self.selected + MAX_VISIBLE_ITEMS).min(self.item_count().saturating_sub(1));
                McpKeyResult::Redraw
            }
            KeyCode::Enter => {
                self.activate(ui_tx);
                McpKeyResult::Redraw
            }
            _ => McpKeyResult::Ignored,
        }
    }

    fn handle_sign_in_key(&mut self, key: KeyEvent) -> McpKeyResult {
        let Some(sign_in) = &mut self.sign_in else {
            return McpKeyResult::Close;
        };
        match key.code {
            KeyCode::Esc => {
                if let Some(sender) = sign_in
                    .redirect
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    let _ = sender.send(None);
                }
                self.sign_in = None;
                self.screen = match &self.screen {
                    Screen::SignIn(name) => Screen::Server(name.clone()),
                    _ => Screen::Servers,
                };
                McpKeyResult::Redraw
            }
            KeyCode::Enter => {
                let value = sign_in.input.trim().to_string();
                let sender = sign_in
                    .redirect
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if let Some(sender) = sender {
                    let _ = sender.send(if value.is_empty() { None } else { Some(value) });
                    sign_in.message = "Connecting…".to_string();
                    sign_in.input.clear();
                }
                McpKeyResult::Redraw
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                sign_in.input.push(character);
                McpKeyResult::Redraw
            }
            KeyCode::Backspace => {
                sign_in.input.pop();
                McpKeyResult::Redraw
            }
            _ => McpKeyResult::Ignored,
        }
    }

    fn back(&mut self) -> McpKeyResult {
        match &self.screen {
            Screen::Servers => McpKeyResult::Close,
            Screen::Server(_) => {
                self.screen = Screen::Servers;
                self.selected = 0;
                McpKeyResult::Redraw
            }
            Screen::Tools(name) | Screen::Exposure(name) => {
                let name = name.clone();
                self.screen = Screen::Server(name);
                self.selected = 0;
                McpKeyResult::Redraw
            }
            Screen::SignIn(name) => {
                let name = name.clone();
                self.screen = Screen::Server(name);
                self.selected = 0;
                McpKeyResult::Redraw
            }
        }
    }

    fn activate(&mut self, ui_tx: &UnboundedSender<UiAction>) {
        match self.screen.clone() {
            Screen::Servers => {
                if let Some(server) = self.server_at_selected() {
                    self.screen = Screen::Server(server.entry.name.clone());
                    self.selected = 0;
                }
            }
            Screen::Server(name) => {
                let Some(server) = self.find(&name) else {
                    return;
                };
                let actions = server_actions(server);
                let Some(action) = actions.get(self.selected) else {
                    return;
                };
                match action.kind {
                    ServerActionKind::SignIn => self.start_sign_in(&name, ui_tx),
                    ServerActionKind::Tools => {
                        self.screen = Screen::Tools(name);
                        self.selected = 0;
                    }
                    ServerActionKind::Reconnect => self.reconnect(&name, ui_tx),
                    ServerActionKind::SignOut => self.sign_out(&name, ui_tx),
                    ServerActionKind::Exposure => {
                        self.screen = Screen::Exposure(name);
                        self.selected = 0;
                    }
                    ServerActionKind::SetEnabled(enabled) => {
                        self.set_enabled(&name, enabled, ui_tx)
                    }
                    ServerActionKind::SetEnabledInProject(enabled) => {
                        self.set_enabled_in_project(&name, enabled, ui_tx)
                    }
                }
            }
            Screen::Tools(name) => {
                self.screen = Screen::Server(name);
                self.selected = 0;
            }
            Screen::Exposure(name) => {
                let options = [
                    McpExposure::Codemode,
                    McpExposure::Deferred,
                    McpExposure::Direct,
                    McpExposure::Hidden,
                ];
                let Some(exposure) = options.get(self.selected).copied() else {
                    return;
                };
                self.set_exposure(&name, exposure);
                self.screen = Screen::Server(name);
                self.selected = 0;
            }
            Screen::SignIn(_) => {}
        }
    }

    fn start_sign_in(&mut self, name: &str, ui_tx: &UnboundedSender<UiAction>) {
        let Some(server) = self.find(name) else {
            return;
        };
        let Some(connection) = server.connection.clone() else {
            return;
        };
        let Ok(url) = server_url(&server.entry) else {
            if let Some(server) = self.find_mut(name) {
                server.message = Some("MCP server has no valid URL.".to_string());
            }
            return;
        };
        let settings = match McpOAuthSettings::from_config(
            server.entry.config.oauth.as_ref(),
            &server.entry.name,
        ) {
            Ok(settings) => settings,
            Err(error) => {
                if let Some(server) = self.find_mut(name) {
                    server.message = Some(error);
                }
                return;
            }
        };
        let store = McpOAuthCredentialStore::new(std::path::Path::new(&self.agent_dir))
            .for_server(name, url.as_str());
        let redirect: Arc<Mutex<Option<oneshot::Sender<Option<String>>>>> =
            Arc::new(Mutex::new(None));
        self.sign_in = Some(SignInUi {
            url: None,
            message: "Contacting the authorization server…".to_string(),
            input: String::new(),
            redirect: Arc::clone(&redirect),
        });
        self.screen = Screen::SignIn(name.to_string());
        let name = name.to_string();
        let ui_tx = ui_tx.clone();
        tokio::spawn(async move {
            let prompt = McpSignInPrompt {
                show_authorization_url: {
                    let name = name.clone();
                    let ui_tx = ui_tx.clone();
                    Arc::new(move |authorization_url| {
                        let _ = ui_tx.send(UiAction::Mcp(McpUiEvent::SignInUrl {
                            name: name.clone(),
                            url: authorization_url.to_string(),
                        }));
                        crate::utils::open_browser::open_browser(authorization_url.as_str());
                    })
                },
                prompt_for_redirect_url: Arc::new(move |_cancel| {
                    let redirect = Arc::clone(&redirect);
                    Box::pin(async move {
                        let (sender, receiver) = oneshot::channel();
                        *redirect
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sender);
                        receiver.await.ok().flatten()
                    })
                }),
            };
            let result =
                mcp_oauth::sign_in_mcp_server(&url, store, &settings, None, prompt, None).await;
            let ok = result.is_ok();
            let message = match result {
                Ok(()) => {
                    let _ = connection.reconnect().await;
                    format!("Signed in to MCP server \"{name}\".")
                }
                Err(error) => format!("Sign-in failed: {error}"),
            };
            let _ = ui_tx.send(UiAction::Mcp(McpUiEvent::SignInDone { name, message, ok }));
        });
    }

    fn reconnect(&mut self, name: &str, ui_tx: &UnboundedSender<UiAction>) {
        let Some(server) = self.find(name) else {
            return;
        };
        let Some(connection) = server.connection.clone() else {
            return;
        };
        let name = name.to_string();
        let ui_tx = ui_tx.clone();
        tokio::spawn(async move {
            let message = match connection.reconnect().await {
                Ok(()) => format!(
                    "Reconnected to MCP server \"{name}\" ({} tools).",
                    connection.tools().len()
                ),
                Err(error) => error,
            };
            let state = connection.state();
            let tools = connection.tools();
            let _ = ui_tx.send(UiAction::Mcp(McpUiEvent::Connected {
                name: name.clone(),
                state,
                tools,
                has_resources: connection.has_resources(),
                instructions: connection.instructions(),
                error: connection.error(),
            }));
            let _ = ui_tx.send(UiAction::Mcp(McpUiEvent::ActionDone { name, message }));
        });
    }

    fn sign_out(&mut self, name: &str, ui_tx: &UnboundedSender<UiAction>) {
        let Some(server) = self.find(name) else {
            return;
        };
        let Ok(url) = server_url(&server.entry) else {
            return;
        };
        let store = McpOAuthCredentialStore::new(std::path::Path::new(&self.agent_dir))
            .for_server(name, url.as_str());
        let removed = store.remove();
        let message = if removed {
            format!("Signed out of MCP server \"{name}\".")
        } else {
            format!("No stored credentials for MCP server \"{name}\".")
        };
        if let Some(server) = self.find_mut(name) {
            server.state = McpServerState::NeedsAuth;
            server.message = Some(message.clone());
        }
        let _ = ui_tx.send(UiAction::Mcp(McpUiEvent::ActionDone {
            name: name.to_string(),
            message,
        }));
    }

    fn set_enabled(&mut self, name: &str, enabled: bool, ui_tx: &UnboundedSender<UiAction>) {
        let Some(server) = self.find(name) else {
            return;
        };
        let path = server
            .entry
            .override_path
            .clone()
            .unwrap_or_else(|| server.entry.source.clone());
        let allow_override = server.entry.override_path.is_some();
        self.apply_enabled(name, enabled, path, allow_override, ui_tx);
    }

    fn set_enabled_in_project(
        &mut self,
        name: &str,
        enabled: bool,
        ui_tx: &UnboundedSender<UiAction>,
    ) {
        let path = mcp_config::project_config_path(std::path::Path::new(&self.cwd));
        self.apply_enabled(name, enabled, path, true, ui_tx);
    }

    fn apply_enabled(
        &mut self,
        name: &str,
        enabled: bool,
        path: std::path::PathBuf,
        allow_override: bool,
        ui_tx: &UnboundedSender<UiAction>,
    ) {
        let cwd = self.cwd.clone();
        let agent_dir = self.agent_dir.clone();
        let patch = McpServerConfigPatch {
            enabled: Some(enabled),
            exposure: None,
        };
        let message = match mcp_config::update_server(&path, name, &patch, allow_override) {
            Ok(_) => {
                if enabled {
                    format!("Enabled MCP server \"{name}\".")
                } else {
                    format!("Disabled MCP server \"{name}\".")
                }
            }
            Err(error) => format!("Could not update {name}: {error}"),
        };
        if let Some(server) = self.find_mut(name) {
            server.entry.config.enabled = Some(enabled);
            if allow_override {
                server.entry.override_path = Some(path);
            }
            server.message = Some(message.clone());
            if !enabled {
                if let Some(connection) = &server.connection {
                    let connection = Arc::clone(connection);
                    tokio::spawn(async move { connection.close().await });
                }
                server.connection = None;
                server.state = McpServerState::Closed;
            } else if server.connection.is_none() {
                let connection = Arc::new(McpServerConnection::with_agent_dir(
                    server.entry.clone(),
                    &cwd,
                    &agent_dir,
                ));
                spawn_connect(Arc::clone(&connection), ui_tx.clone());
                server.connection = Some(connection);
                server.state = McpServerState::Connecting;
                server.error = None;
                self.owned.insert(name.to_string());
            }
        }
        let _ = ui_tx.send(UiAction::Mcp(McpUiEvent::ConfigChanged {
            name: name.to_string(),
            message,
        }));
    }

    fn set_exposure(&mut self, name: &str, exposure: McpExposure) {
        let Some(server) = self.find(name) else {
            return;
        };
        let path = server
            .entry
            .override_path
            .clone()
            .unwrap_or_else(|| server.entry.source.clone());
        let patch = McpServerConfigPatch {
            enabled: None,
            exposure: Some(exposure),
        };
        let message = match mcp_config::update_server(
            &path,
            name,
            &patch,
            server.entry.override_path.is_some(),
        ) {
            Ok(_) => format!("Exposure of \"{name}\" set to {exposure}."),
            Err(error) => format!("Could not update {name}: {error}"),
        };
        if let Some(server) = self.find_mut(name) {
            server.entry.config.exposure = Some(exposure);
            server.message = Some(message);
        }
    }

    /// Apply an async event.
    pub fn on_event(&mut self, event: McpUiEvent) {
        match event {
            McpUiEvent::Connected {
                name,
                state,
                tools,
                has_resources,
                instructions,
                error,
            } => {
                if let Some(server) = self.find_mut(&name) {
                    server.state = state;
                    server.tools = tools;
                    server.has_resources = has_resources;
                    server.instructions = instructions;
                    server.error = error;
                    if state == McpServerState::Connected {
                        server.message = None;
                    }
                }
            }
            McpUiEvent::ActionDone { name, message } => {
                if let Some(server) = self.find_mut(&name) {
                    server.message = Some(message);
                }
            }
            McpUiEvent::SignInUrl { name, url } => {
                if let Some(sign_in) = &mut self.sign_in {
                    if matches!(&self.screen, Screen::SignIn(current) if current == &name) {
                        sign_in.url = Some(url);
                        sign_in.message = "Waiting for the browser callback…".to_string();
                    }
                }
            }
            McpUiEvent::SignInDone { name, message, ok } => {
                self.sign_in = None;
                self.screen = Screen::Server(name.clone());
                self.selected = 0;
                if let Some(server) = self.find_mut(&name) {
                    server.message = Some(message);
                    if !ok {
                        server.state = McpServerState::NeedsAuth;
                    }
                }
            }
            McpUiEvent::ConfigChanged { name, message } => {
                if let Some(server) = self.find_mut(&name) {
                    server.message = Some(message);
                }
            }
        }
    }

    /// Close all manager-owned connections.
    pub async fn close(&self) {
        for server in &self.servers {
            if !self.owned.contains(&server.entry.name) {
                continue;
            }
            if let Some(connection) = &server.connection {
                connection.close().await;
            }
        }
    }
}

fn spawn_connect(connection: Arc<McpServerConnection>, ui_tx: UnboundedSender<UiAction>) {
    tokio::spawn(async move {
        let state = match connection.get_client().await {
            Ok(_) => McpServerState::Connected,
            Err(_) => connection.state(),
        };
        let _ = ui_tx.send(UiAction::Mcp(McpUiEvent::Connected {
            name: connection.name().to_string(),
            state,
            tools: connection.tools(),
            has_resources: connection.has_resources(),
            instructions: connection.instructions(),
            error: connection.error(),
        }));
    });
}

struct ServerAction {
    kind: ServerActionKind,
    label: String,
    description: Option<String>,
}

enum ServerActionKind {
    SignIn,
    Tools,
    Reconnect,
    SignOut,
    Exposure,
    SetEnabled(bool),
    SetEnabledInProject(bool),
}

fn server_actions(server: &ServerUi) -> Vec<ServerAction> {
    let saved = if server.entry.scope.as_str() == "extension" {
        "for this session".to_string()
    } else if server.entry.override_path.is_some() {
        "saved to the project mcp.json".to_string()
    } else {
        format!("saved to the {} mcp.json", server.entry.scope.as_str())
    };
    let in_project =
        server.entry.scope.as_str() == "global" && server.entry.override_path.is_none();
    let mut actions = Vec::new();
    if !server.enabled() {
        actions.push(ServerAction {
            kind: ServerActionKind::SetEnabled(true),
            label: "Enable".to_string(),
            description: Some(saved),
        });
        if in_project {
            actions.push(ServerAction {
                kind: ServerActionKind::SetEnabledInProject(true),
                label: "Enable in this project".to_string(),
                description: Some("saved to the project mcp.json".to_string()),
            });
        }
        return actions;
    }
    if server.state == McpServerState::NeedsAuth {
        actions.push(ServerAction {
            kind: ServerActionKind::SignIn,
            label: "Sign in".to_string(),
            description: Some("opens the browser".to_string()),
        });
    }
    if server.state == McpServerState::Connected {
        actions.push(ServerAction {
            kind: ServerActionKind::Tools,
            label: "Tools".to_string(),
            description: Some(format!("{} offered", server.tools.len())),
        });
    }
    actions.push(ServerAction {
        kind: ServerActionKind::Reconnect,
        label: "Reconnect".to_string(),
        description: None,
    });
    if server.state == McpServerState::Connected
        && mcp_oauth::server_uses_oauth(&server.entry.config)
    {
        actions.push(ServerAction {
            kind: ServerActionKind::SignOut,
            label: "Sign out".to_string(),
            description: Some("deletes the stored credentials".to_string()),
        });
    }
    actions.push(ServerAction {
        kind: ServerActionKind::Exposure,
        label: "Exposure".to_string(),
        description: Some(server.entry.config.exposure().to_string()),
    });
    actions.push(ServerAction {
        kind: ServerActionKind::SetEnabled(false),
        label: "Disable".to_string(),
        description: Some(saved),
    });
    if in_project {
        actions.push(ServerAction {
            kind: ServerActionKind::SetEnabledInProject(false),
            label: "Disable in this project".to_string(),
            description: Some("saved to the project mcp.json".to_string()),
        });
    }
    actions
}

fn state_summary(server: &ServerUi) -> String {
    if !server.enabled() {
        return "disabled".to_string();
    }
    match server.state {
        McpServerState::Connected => format!(
            "connected · {} tools{}",
            server.tools.len(),
            if server.has_resources {
                " · resources"
            } else {
                ""
            }
        ),
        McpServerState::NeedsAuth => "needs sign-in".to_string(),
        McpServerState::Failed => "failed".to_string(),
        McpServerState::Connecting => "connecting…".to_string(),
        McpServerState::Disconnected => "disconnected".to_string(),
        McpServerState::Closed => "closed".to_string(),
    }
}

fn server_url(entry: &LoadedServer) -> Result<url::Url, String> {
    entry
        .config
        .url
        .as_deref()
        .and_then(|url| url::Url::parse(url).ok())
        .ok_or_else(|| format!("MCP server \"{}\" has no valid URL", entry.name))
}

fn describe_transport(entry: &LoadedServer) -> String {
    if let Some(command) = &entry.config.command {
        let mut parts = vec![command.clone()];
        parts.extend(entry.config.args.clone().unwrap_or_default());
        return parts.join(" ");
    }
    let kind = if entry.config.kind.as_deref() == Some("sse") {
        "sse"
    } else {
        "http"
    };
    format!("{kind} {}", entry.config.url.as_deref().unwrap_or_default())
}

fn exposure_description(exposure: McpExposure) -> &'static str {
    match exposure {
        McpExposure::Codemode => "called from codemode scripts, which find them with searchTools()",
        McpExposure::Deferred => {
            "not declared until tool_search loads them, then called directly; no codemode needed"
        }
        McpExposure::Direct => "declared to the model like built-in tools",
        McpExposure::Hidden => "unreachable",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn server(name: &str) -> ServerUi {
        let config = crate::core::mcp_servers::validate_mcp_server_config(
            name,
            &serde_json::json!({"command": "true", "exposure": "codemode"}),
        )
        .unwrap();
        ServerUi {
            entry: LoadedServer {
                name: name.to_string(),
                config,
                source: std::path::PathBuf::from("mcp.json"),
                scope: crate::core::mcp_servers::McpScope::Global,
                override_path: None,
            },
            connection: None,
            state: McpServerState::Connected,
            error: None,
            tools: Vec::new(),
            has_resources: false,
            instructions: None,
            message: None,
        }
    }

    fn state() -> McpUiState {
        McpUiState {
            cwd: ".".to_string(),
            agent_dir: ".".to_string(),
            servers: vec![server("docs"), server("tools")],
            config_errors: Vec::new(),
            screen: Screen::Servers,
            selected: 0,
            sign_in: None,
            owned: std::collections::HashSet::new(),
        }
    }

    #[test]
    fn server_list_shows_state_and_exposure() {
        let panel = state().panel();
        assert_eq!(panel.title, "MCP servers");
        assert_eq!(panel.items.len(), 2);
        assert!(panel.items[0]
            .description
            .as_deref()
            .unwrap()
            .contains("connected"));
        assert!(panel.items[0]
            .description
            .as_deref()
            .unwrap()
            .contains("codemode"));
    }

    #[test]
    fn enter_opens_server_and_exposure_screens() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = state();
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &tx);
        assert_eq!(state.screen, Screen::Server("docs".to_string()));
        // Connected server actions: Tools, Reconnect, Exposure, Disable.
        state.selected = 2;
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &tx);
        assert_eq!(state.screen, Screen::Exposure("docs".to_string()));
        let panel = state.panel();
        assert!(panel
            .items
            .iter()
            .any(|item| item.label.starts_with("✓ codemode")));
    }

    #[test]
    fn sign_in_input_edits_and_submits() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = state();
        let (sender, receiver) = oneshot::channel();
        state.sign_in = Some(SignInUi {
            url: Some("https://example.com/authorize".to_string()),
            message: String::new(),
            input: String::new(),
            redirect: Arc::new(Mutex::new(Some(sender))),
        });
        state.screen = Screen::SignIn("docs".to_string());
        for character in "http://127.0.0.1/cb?code=x".chars() {
            state.handle_key(
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                &tx,
            );
        }
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &tx);
        assert_eq!(
            futures::executor::block_on(receiver).unwrap(),
            Some("http://127.0.0.1/cb?code=x".to_string())
        );
    }
}
