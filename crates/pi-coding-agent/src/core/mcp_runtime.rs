//! MCP server connections over the new `pi-mcp` client.
//!
//! Port of `packages/coding-agent/src/extensions/mcp/runtime.ts`. This module
//! deliberately returns `pi_mcp` values; `mcp_tools` adapts them to pi's tool
//! pipeline.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use pi_mcp::oauth::OAuthError;
use pi_mcp::{
    CallToolResult, ListResourceTemplatesResult, ListResourcesResult, McpClient,
    McpClientOptions, McpError, McpRequestOptions, McpTransport, ReadResourceResult, Root,
    RootsProvider, StreamableHttpTransport, StreamableHttpTransportOptions, Subscription, Tool,
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::VERSION;
use crate::core::mcp_config::LoadedServer;
use crate::core::mcp_servers::McpServerConfig;
use crate::core::resolve_config_value::{resolve_config_value_or_throw, resolve_headers_or_throw};
use pi_mcp::transports::stdio::{StdioStderr, StdioTransport, StdioTransportOptions};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_TIMEOUT_SECONDS: f64 = 60.0;
const STDERR_TAIL_CHARS: usize = 2_000;

/// Connection state, matching the TS `ServerState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerState {
    /// Connection in progress.
    Connecting,
    /// Connected and initialized.
    Connected,
    /// The transport dropped; the next call reconnects.
    Disconnected,
    /// The server requires an OAuth sign-in.
    NeedsAuth,
    /// The connection failed.
    Failed,
    /// Closed by the session.
    Closed,
}

impl McpServerState {
    /// Human-readable state for `/mcp` and `pi-rs mcp list`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::NeedsAuth => "needs-auth",
            Self::Failed => "failed",
            Self::Closed => "closed",
        }
    }
}

struct Inner {
    entry: LoadedServer,
    cwd: String,
    agent_dir: String,
    log: Arc<crate::core::mcp_log::McpServerLog>,
    subscriptions: Mutex<Vec<Subscription>>,
    state: Mutex<McpServerState>,
    error: Mutex<Option<String>>,
    tools: Mutex<Vec<Tool>>,
    has_resources: AtomicBool,
    instructions: Mutex<Option<String>>,
    client: tokio::sync::Mutex<Option<Arc<McpClient>>>,
    connect_lock: tokio::sync::Mutex<()>,
    shutdown: CancellationToken,
}

impl Inner {
    fn state(&self) -> McpServerState {
        *self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_state(&self, state: McpServerState) {
        *self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = state;
    }

    fn set_error(&self, error: Option<String>) {
        *self.error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
    }

    fn tools(&self) -> MutexGuard<'_, Vec<Tool>> {
        self.tools
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// One configured MCP server.
#[derive(Clone)]
pub struct McpServerConnection {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for McpServerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerConnection")
            .field("name", &self.inner.entry.name)
            .field("state", &self.inner.state())
            .finish()
    }
}

impl McpServerConnection {
    /// Create a connection; no transport is opened until [`Self::get_client`].
    pub fn new(entry: LoadedServer, cwd: impl Into<String>) -> Self {
        let agent_dir = crate::config::get_agent_dir()
            .to_string_lossy()
            .to_string();
        Self::with_agent_dir(entry, cwd, agent_dir)
    }

    /// Create a connection with an explicit agent directory (tests, RPC).
    pub fn with_agent_dir(
        entry: LoadedServer,
        cwd: impl Into<String>,
        agent_dir: impl Into<String>,
    ) -> Self {
        let agent_dir = agent_dir.into();
        let log = Arc::new(crate::core::mcp_log::McpServerLog::new(
            std::path::Path::new(&agent_dir).join("mcp.log"),
        ));
        Self {
            inner: Arc::new(Inner {
                entry,
                cwd: cwd.into(),
                agent_dir,
                log,
                subscriptions: Mutex::new(Vec::new()),
                state: Mutex::new(McpServerState::Connecting),
                error: Mutex::new(None),
                tools: Mutex::new(Vec::new()),
                has_resources: AtomicBool::new(false),
                instructions: Mutex::new(None),
                client: tokio::sync::Mutex::new(None),
                connect_lock: tokio::sync::Mutex::new(()),
                shutdown: CancellationToken::new(),
            }),
        }
    }

    /// Server name.
    pub fn name(&self) -> &str {
        &self.inner.entry.name
    }

    /// Parsed config.
    pub fn entry(&self) -> &LoadedServer {
        &self.inner.entry
    }

    /// Current state.
    pub fn state(&self) -> McpServerState {
        self.inner.state()
    }

    /// Last connection error.
    pub fn error(&self) -> Option<String> {
        self.inner
            .error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Tools enumerated at connect time.
    pub fn tools(&self) -> Vec<Tool> {
        self.inner.tools().clone()
    }

    /// Whether the server offers resources.
    pub fn has_resources(&self) -> bool {
        self.inner.has_resources.load(Ordering::SeqCst)
    }

    /// Server instructions from `initialize`.
    pub fn instructions(&self) -> Option<String> {
        self.inner
            .instructions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Per-request timeout.
    pub fn timeout(&self) -> Duration {
        let seconds = if self.inner.entry.config.timeout.is_some() {
            self.inner.entry.config.timeout_seconds()
        } else {
            DEFAULT_TIMEOUT_SECONDS
        };
        Duration::from_secs_f64(seconds.max(0.1))
    }

    fn config(&self) -> &McpServerConfig {
        &self.inner.entry.config
    }

    /// Connect if needed and return the client.
    pub async fn get_client(&self) -> Result<Arc<McpClient>, String> {
        if self.inner.shutdown.is_cancelled() {
            return Err(format!("MCP server \"{}\" is shut down", self.name()));
        }
        if let Some(client) = self.inner.client.lock().await.clone() {
            if client.connection_state() == pi_mcp::client::ClientState::Connected {
                return Ok(client);
            }
        }
        let _guard = self.inner.connect_lock.lock().await;
        if self.inner.shutdown.is_cancelled() {
            return Err(format!("MCP server \"{}\" is shut down", self.name()));
        }
        if let Some(client) = self.inner.client.lock().await.clone() {
            if client.connection_state() == pi_mcp::client::ClientState::Connected {
                return Ok(client);
            }
        }
        self.inner.set_state(McpServerState::Connecting);
        self.inner.set_error(None);
        match tokio::time::timeout(CONNECT_TIMEOUT, self.connect_once()).await {
            Ok(Ok(())) => {
                let client = self
                    .inner
                    .client
                    .lock()
                    .await
                    .clone()
                    .ok_or_else(|| format!("MCP server \"{}\" did not connect", self.name()))?;
                Ok(client)
            }
            Ok(Err(error)) => {
                let needs_auth = error.contains("requires sign-in");
                self.inner.set_state(if needs_auth {
                    McpServerState::NeedsAuth
                } else {
                    McpServerState::Failed
                });
                self.inner.set_error(Some(error.clone()));
                Err(format!(
                    "MCP server \"{}\" failed to connect: {error}",
                    self.name()
                ))
            }
            Err(_) => {
                self.inner.set_state(McpServerState::Failed);
                let error = format!("timed out after {}s", CONNECT_TIMEOUT.as_secs());
                self.inner.set_error(Some(error.clone()));
                Err(format!("MCP server \"{}\" failed to connect: {error}", self.name()))
            }
        }
    }

    async fn connect_once(&self) -> Result<(), String> {
        let transport = self.create_transport()?;
        let roots = vec![Root {
            uri: Url::from_directory_path(&self.inner.cwd)
                .map(|url| url.to_string())
                .unwrap_or_else(|_| format!("file://{}", self.inner.cwd)),
            name: PathBuf::from(&self.inner.cwd)
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
        }];
        let options = McpClientOptions {
            name: "pi-rs".to_string(),
            version: VERSION.to_string(),
            title: None,
            capabilities: Default::default(),
            protocol_version: None,
            request_timeout: Some(self.timeout()),
            roots: Some(RootsProvider::Static(roots)),
        };
        let client = Arc::new(McpClient::new(options));
        if let Err(error) = client.connect(transport).await {
            if matches!(&error, McpError::AuthRequired { .. })
                || matches!(&error, McpError::OAuth(error) if matches!(**error, OAuthError::AuthorizationRequired))
            {
                return Err(format!(
                    "requires sign-in; run /mcp or `pi-rs mcp login {}`",
                    self.name()
                ));
            }
            return Err(error.to_string());
        }
        let capabilities = client.server_capabilities().unwrap_or_default();
        let tools = if capabilities.tools.is_some() {
            client
                .list_tools(McpRequestOptions::default())
                .await
                .map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
        *self.inner.client.lock().await = Some(Arc::clone(&client));
        self.inner.set_state(McpServerState::Connected);
        self.inner.set_error(None);
        *self.inner.tools() = tools;
        self.inner
            .has_resources
            .store(capabilities.resources.is_some(), Ordering::SeqCst);
        *self
            .inner
            .instructions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = client.instructions();
        {
            let log = Arc::clone(&self.inner.log);
            let name = self.name().to_string();
            let subscription = client.on_notification(
                "notifications/message",
                Arc::new(move |params| log.write(&name, params.as_ref())),
            );
            self.inner
                .subscriptions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(subscription);
        }
        // Refresh tools when the server announces changes.
        {
            let connection = self.clone();
            let notification_client = Arc::clone(&client);
            let _subscription = client.on_notification(
                "notifications/tools/list_changed",
                Arc::new(move |_| {
                    let connection = connection.clone();
                    let client = Arc::clone(&notification_client);
                    tokio::spawn(async move {
                        if let Ok(tools) = client.list_tools(McpRequestOptions::default()).await {
                            *connection.inner.tools() = tools;
                        }
                    });
                }),
            );
            // Keep the subscription alive until the connection closes.
            self.inner
                .subscriptions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(_subscription);
        }
        Ok(())
    }

    fn create_transport(&self) -> Result<Arc<dyn McpTransport>, String> {
        let config = self.config();
        if let Some(command) = &config.command {
            let env = match &config.env {
                Some(env) => {
                    let mut resolved = BTreeMap::new();
                    for (key, value) in env {
                        resolved.insert(
                            key.clone(),
                            resolve_config_value_or_throw(
                                value,
                                &format!("MCP server \"{}\" env \"{key}\"", self.name()),
                            )?,
                        );
                    }
                    resolved
                }
                None => BTreeMap::new(),
            };
            let cwd = config
                .cwd
                .as_deref()
                .map(|cwd| PathBuf::from(&self.inner.cwd).join(expand_home(cwd)))
                .unwrap_or_else(|| PathBuf::from(&self.inner.cwd));
            let transport = StdioTransport::new(StdioTransportOptions {
                command: expand_home(command),
                args: config.args.clone().unwrap_or_default(),
                cwd: Some(cwd),
                env,
                inherit_env: true,
                stderr: StdioStderr::Pipe,
                on_stderr: None,
                max_message_bytes: 16 * 1024 * 1024,
                max_stderr_bytes: 64 * 1024,
                close_timeout: Duration::from_secs(2),
            });
            return Ok(transport as Arc<dyn McpTransport>);
        }
        if config.kind.as_deref() == Some("sse") {
            let url = Url::parse(config.url.as_deref().unwrap_or_default())
                .map_err(|error| format!("invalid MCP SSE URL: {error}"))?;
            let mut options = pi_mcp::transports::legacy_sse::LegacySseTransportOptions::new(url);
            options.headers = resolve_headers(config, self.name())?;
            let transport = pi_mcp::transports::legacy_sse::LegacySseTransport::new(options);
            return Ok(transport as Arc<dyn McpTransport>);
        }
        let Some(url) = &config.url else {
            return Err(format!(
                "MCP server \"{}\" has neither command nor url",
                self.name()
            ));
        };
        let headers = resolve_headers(config, self.name())?;
        let url = Url::parse(url).map_err(|error| format!("invalid MCP server URL: {error}"))?;
        let mut options = StreamableHttpTransportOptions::new(url.clone());
        options.headers = headers;
        if crate::core::mcp_oauth::server_uses_oauth(config) {
            let settings = crate::core::mcp_oauth::McpOAuthSettings::from_config(
                config.oauth.as_ref(),
                self.name(),
            )?;
            let store = crate::core::mcp_oauth::McpOAuthCredentialStore::new(std::path::Path::new(
                &self.inner.agent_dir,
            ))
            .for_server(self.name(), url.as_str());
            options.auth = Some(crate::core::mcp_oauth::create_mcp_auth_provider(
                self.name(),
                &url,
                settings,
                store,
            ));
        }
        let transport = StreamableHttpTransport::new(options);
        Ok(transport as Arc<dyn McpTransport>)
    }

    /// Call a tool, retrying once on an expired session.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
        cancel: Option<CancellationToken>,
    ) -> Result<CallToolResult, String> {
        let mut attempt = 0;
        loop {
            let client = self.get_client().await?;
            let options = McpRequestOptions {
                cancel: cancel.clone(),
                timeout: Some(self.timeout()),
                on_progress: None,
            };
            match client.call_tool(name, Some(arguments.clone()), options).await {
                Ok(result) => {
                    self.inner.set_state(McpServerState::Connected);
                    return Ok(result);
                }
                Err(McpError::SessionExpired { .. }) if attempt == 0 => {
                    attempt += 1;
                    *self.inner.client.lock().await = None;
                    continue;
                }
                Err(McpError::AuthRequired { .. })
                | Err(McpError::OAuth(_)) => {
                    self.inner.set_state(McpServerState::NeedsAuth);
                    return Err(format!(
                        "MCP server \"{}\" requires sign-in; run /mcp or `pi-rs mcp login {}`",
                        self.name(),
                        self.name()
                    ));
                }
                Err(error) => return Err(format!("MCP call \"{name}\" failed: {error}")),
            }
        }
    }

    /// List every resource.
    pub async fn list_resources(&self) -> Result<Vec<pi_mcp::Resource>, String> {
        let client = self.get_client().await?;
        client
            .list_resources(McpRequestOptions {
                timeout: Some(self.timeout()),
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())
    }

    /// One page of resources.
    pub async fn list_resources_page(
        &self,
        cursor: Option<String>,
    ) -> Result<ListResourcesResult, String> {
        let client = self.get_client().await?;
        client
            .list_resources_page(
                cursor,
                McpRequestOptions {
                    timeout: Some(self.timeout()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| error.to_string())
    }

    /// List every resource template.
    pub async fn list_resource_templates(&self) -> Result<Vec<pi_mcp::ResourceTemplate>, String> {
        let client = self.get_client().await?;
        client
            .list_resource_templates(McpRequestOptions {
                timeout: Some(self.timeout()),
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())
    }

    /// One page of resource templates.
    pub async fn list_resource_templates_page(
        &self,
        cursor: Option<String>,
    ) -> Result<ListResourceTemplatesResult, String> {
        let client = self.get_client().await?;
        client
            .list_resource_templates_page(
                cursor,
                McpRequestOptions {
                    timeout: Some(self.timeout()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| error.to_string())
    }

    /// Read a resource.
    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult, String> {
        let client = self.get_client().await?;
        client
            .read_resource(
                uri,
                McpRequestOptions {
                    timeout: Some(self.timeout()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| error.to_string())
    }

    /// Close the connection.
    pub async fn close(&self) {
        self.inner.shutdown.cancel();
        self.inner.set_state(McpServerState::Closed);
        self.inner
            .subscriptions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        if let Some(client) = self.inner.client.lock().await.take() {
            let _ = client.close().await;
        }
    }

    /// Reconnect with fresh credentials.
    pub async fn reconnect(&self) -> Result<(), String> {
        if let Some(client) = self.inner.client.lock().await.take() {
            let _ = client.close().await;
        }
        self.get_client().await.map(|_| ())
    }
}

fn resolve_headers(
    config: &McpServerConfig,
    name: &str,
) -> Result<BTreeMap<String, String>, String> {
    match &config.headers {
        Some(headers) => {
            let source: std::collections::HashMap<String, String> = headers
                .iter()
                .map(|(header, value)| (header.clone(), value.clone()))
                .collect();
            Ok(resolve_headers_or_throw(Some(&source), &format!("MCP server \"{name}\" headers"))?
                .unwrap_or_default()
                .into_iter()
                .collect())
        }
        None => Ok(BTreeMap::new()),
    }
}

fn expand_home(value: &str) -> String {    let Some(home) = dirs::home_dir() else {
        return value.to_string();
    };
    if value == "~" {
        return home.to_string_lossy().to_string();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home.join(rest).to_string_lossy().to_string();
    }
    value.to_string()
}

/// Read the stderr tail of a stdio connection for diagnostics.
pub fn stderr_tail(value: &str) -> Option<String> {
    let tail: String = value
        .chars()
        .rev()
        .take(STDERR_TAIL_CHARS)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let tail = tail.trim();
    if tail.is_empty() {
        None
    } else {
        Some(tail.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::core::mcp_servers::{
        validate_mcp_server_config, LoadedServer, McpScope,
    };
    use serde_json::json;

    const SERVER_SCRIPT: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([^,}]+).*/\1/')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"sh","version":"1"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([^,}]+).*/\1/')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}\n' "$id"
      ;;
  esac
done
"#;

    #[tokio::test]
    async fn stdio_connection_lists_tools() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.sh");
        std::fs::write(&script, SERVER_SCRIPT).unwrap();
        let config = validate_mcp_server_config(
            "echo",
            &json!({"command": "/bin/sh", "args": [script]}),
        )
        .unwrap();
        let entry = LoadedServer {
            name: "echo".to_string(),
            config,
            source: dir.path().join("mcp.json"),
            scope: McpScope::Global,
            override_path: None,
        };
        let cwd = dir.path().to_string_lossy().to_string();
        let connection = McpServerConnection::with_agent_dir(entry, &cwd, &cwd);
        connection.get_client().await.expect("stdio server connects");
        assert_eq!(connection.tools().len(), 1);
        assert_eq!(connection.tools()[0].name, "echo");
        connection.close().await;
    }
}
