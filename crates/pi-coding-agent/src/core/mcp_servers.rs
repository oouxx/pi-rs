//! MCP server configuration types, validation, and the extension registry.
//!
//! Port of `packages/coding-agent/src/core/mcp-servers.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::config::CONFIG_DIR_NAME;

/// MCP resource tools, which reach every server with resources.
pub const LIST_MCP_RESOURCES_TOOL: &str = "list_mcp_resources";
/// Lists resource templates across servers.
pub const LIST_MCP_RESOURCE_TEMPLATES_TOOL: &str = "list_mcp_resource_templates";
/// Reads one resource from a server.
pub const READ_MCP_RESOURCE_TOOL: &str = "read_mcp_resource";

const MCP_RESOURCE_TOOLS: [&str; 3] = [
    LIST_MCP_RESOURCES_TOOL,
    LIST_MCP_RESOURCE_TEMPLATES_TOOL,
    READ_MCP_RESOURCE_TOOL,
];

/// Whether a tool name comes from MCP.
pub fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with("mcp__") || MCP_RESOURCE_TOOLS.contains(&name)
}

/// How a server's tools are exposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum McpExposure {
    /// Callable from codemode scripts, not declared to the model.
    #[default]
    Codemode,
    /// Loaded by `tool_search` before being declared.
    Deferred,
    /// Declared to the model like built-in tools.
    Direct,
    /// Registered but unreachable.
    Hidden,
}

impl std::fmt::Display for McpExposure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(exposure_name(*self))
    }
}

impl McpExposure {
    /// Parse an exposure name, including the legacy `codemode-deferred` alias.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "codemode" | "codemode-deferred" => Some(Self::Codemode),
            "deferred" => Some(Self::Deferred),
            "direct" => Some(Self::Direct),
            "hidden" => Some(Self::Hidden),
            _ => None,
        }
    }
}

/// OAuth client settings for HTTP servers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthConfig {
    /// Pre-registered client id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Client secret (may reference `${VAR}` / `!cmd`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Fixed callback port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<u16>,
    /// Registered loopback redirect URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    /// Requested scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Client name sent with dynamic registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    /// How the client identifies itself (`auto`, `dcr`, `cimd`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_registration: Option<String>,
    /// Explicit authorization-server metadata document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_server_metadata_url: Option<String>,
}

/// Use a pi provider token instead of OAuth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerAuth {
    /// Provider id.
    pub provider: String,
}

/// One `mcpServers` entry.
///
/// `command` and `url` are mutually exclusive; `type` disambiguates. Keeping
/// the fields flat mirrors the TS duck-typed config and makes read/edit/write
/// round-trips lossless.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Explicit transport type.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Exposure, default `codemode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<McpExposure>,
    /// What the server offers, in a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Per-tool exposure overrides.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub tool_exposure: IndexMap<String, McpExposure>,
    /// Whether the server is enabled. Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Per-request timeout in seconds. Default 60.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
    /// Stdio executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Stdio arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Stdio environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    /// Stdio working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// HTTP URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<IndexMap<String, String>>,
    /// OAuth settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthConfig>,
    /// Provider-token auth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<McpServerAuth>,
}

impl McpServerConfig {
    /// Whether this is a stdio server.
    pub fn is_stdio(&self) -> bool {
        self.command.is_some()
    }

    /// Whether this is an HTTP server.
    pub fn is_http(&self) -> bool {
        self.url.is_some()
    }

    /// Whether the server is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// Exposure of the server's tools by default.
    pub fn exposure(&self) -> McpExposure {
        self.exposure.unwrap_or_default()
    }

    /// Timeout in seconds.
    pub fn timeout_seconds(&self) -> f64 {
        self.timeout.unwrap_or(60.0)
    }

    /// Exposure of one tool.
    pub fn tool_exposure(&self, tool: &str) -> McpExposure {
        if let Some(exposure) = self.tool_exposure.get(tool) {
            return *exposure;
        }
        for (pattern, exposure) in &self.tool_exposure {
            if pattern.contains('*') && tool_pattern_matches(pattern, tool) {
                return *exposure;
            }
        }
        self.exposure()
    }
}

/// Namespace of a server's tools: `mcp__<server>` with `-` replaced by `_`.
pub fn mcp_namespace(server: &str) -> String {
    format!("mcp__{}", server.replace('-', "_"))
}

fn tool_pattern_matches(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = name;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(position) => {
                if index == 0 && position != 0 {
                    return false;
                }
                rest = &rest[position + part.len()..];
            }
            None => return false,
        }
    }
    if let Some(last) = parts.last() {
        if !last.is_empty() && !pattern.ends_with('*') && !name.ends_with(last) {
            return false;
        }
    }
    true
}

/// Whether a tool name matches any exact name or `*` pattern.
pub fn create_tool_name_matcher(entries: &[String]) -> impl Fn(&str) -> bool + '_ {
    let names: std::collections::HashSet<&String> =
        entries.iter().filter(|entry| !entry.contains('*')).collect();
    move |name: &str| {
        names.contains(&name.to_string())
            || entries
                .iter()
                .filter(|entry| entry.contains('*'))
                .any(|pattern| tool_pattern_matches(pattern, name))
    }
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

fn is_string_record(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.values().all(Value::is_string))
}

fn loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// Whether a redirect URI can be served by the loopback callback server.
pub fn is_loopback_redirect_uri(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "http"
            && url.host_str().is_some_and(loopback_host)
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

/// Whether pi's Client ID Metadata Documents list the loopback redirect URI.
pub fn is_client_metadata_document_callback(callback_url: Option<&str>) -> bool {
    match callback_url {
        None => true,
        Some(value) => Url::parse(value).is_ok_and(|url| {
            url.host_str() != Some("[::1]") && url.path() == "/callback"
        }),
    }
}

fn validate_oauth(value: Option<&Value>) -> Result<Option<McpOAuthConfig>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(object) = value.as_object() else {
        return Err("oauth must be an object".to_string());
    };
    let mut oauth = McpOAuthConfig::default();
    if let Some(client_id) = object.get("clientId") {
        if !client_id.is_string() {
            return Err("oauth.clientId must be a string".to_string());
        }
        oauth.client_id = client_id.as_str().map(str::to_string);
    }
    if let Some(client_secret) = object.get("clientSecret") {
        if !client_secret.is_string() {
            return Err("oauth.clientSecret must be a string".to_string());
        }
        oauth.client_secret = client_secret.as_str().map(str::to_string);
    }
    if let Some(port) = object.get("callbackPort") {
        let port = port
            .as_u64()
            .filter(|port| (1..=65535).contains(port))
            .ok_or_else(|| "oauth.callbackPort must be a port number".to_string())?;
        oauth.callback_port = Some(port as u16);
    }
    if let Some(callback_url) = object.get("callbackUrl") {
        let callback_url = callback_url
            .as_str()
            .ok_or_else(|| "oauth.callbackUrl must be a string".to_string())?;
        if !is_loopback_redirect_uri(callback_url) {
            return Err(
                "oauth.callbackUrl must be an http URI on localhost, 127.0.0.1, or [::1] without query or fragment"
                    .to_string(),
            );
        }
        let url = Url::parse(callback_url).map_err(|error| error.to_string())?;
        if let (Some(url_port), Some(port)) = (url.port(), oauth.callback_port) {
            if url_port != port {
                return Err("oauth.callbackUrl and oauth.callbackPort name different ports".to_string());
            }
        }
        oauth.callback_url = Some(callback_url.to_string());
    }
    if let Some(scope) = object.get("scope") {
        if !scope.is_string() {
            return Err("oauth.scope must be a string".to_string());
        }
        oauth.scope = scope.as_str().map(str::to_string);
    }
    if let Some(client_name) = object.get("clientName") {
        let client_name = client_name
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| "oauth.clientName must be a non-empty string".to_string())?;
        oauth.client_name = Some(client_name.to_string());
    }
    if let Some(registration) = object.get("clientRegistration") {
        let registration = registration
            .as_str()
            .ok_or_else(|| "oauth.clientRegistration must be \"auto\", \"dcr\", or \"cimd\"".to_string())?;
        if !matches!(registration, "auto" | "dcr" | "cimd") {
            return Err("oauth.clientRegistration must be \"auto\", \"dcr\", or \"cimd\"".to_string());
        }
        if registration == "cimd" {
            if oauth.client_id.is_some() || oauth.client_name.is_some() {
                return Err(
                    "oauth.clientRegistration \"cimd\" cannot be combined with oauth.clientId or oauth.clientName"
                        .to_string(),
                );
            }
            if !is_client_metadata_document_callback(oauth.callback_url.as_deref()) {
                return Err(
                    "oauth.clientRegistration \"cimd\" requires oauth.callbackUrl on localhost or 127.0.0.1 with path /callback"
                        .to_string(),
                );
            }
        }
        oauth.client_registration = Some(registration.to_string());
    }
    if let Some(metadata_url) = object.get("authServerMetadataUrl") {
        let metadata_url = metadata_url
            .as_str()
            .ok_or_else(|| "oauth.authServerMetadataUrl must be a string".to_string())?;
        let url = Url::parse(metadata_url)
            .map_err(|_| "oauth.authServerMetadataUrl must be a valid URL".to_string())?;
        let secure = url.scheme() == "https"
            || (url.scheme() == "http" && url.host_str().is_some_and(loopback_host));
        if !secure {
            return Err(
                "oauth.authServerMetadataUrl must be an https URL, or http on localhost, 127.0.0.1, or [::1]"
                    .to_string(),
            );
        }
        oauth.auth_server_metadata_url = Some(metadata_url.to_string());
    }
    Ok(Some(oauth))
}

fn validate_auth(value: Option<&Value>, url: &str) -> Result<Option<McpServerAuth>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let provider = value
        .as_object()
        .and_then(|object| object.get("provider"))
        .and_then(Value::as_str)
        .filter(|provider| !provider.is_empty())
        .ok_or_else(|| "auth.provider must be a provider name".to_string())?;
    let parsed = Url::parse(url).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https" && !parsed.host_str().is_some_and(loopback_host) {
        return Err(
            "auth requires an https URL, or http on localhost, 127.0.0.1, or [::1]".to_string(),
        );
    }
    Ok(Some(McpServerAuth {
        provider: provider.to_string(),
    }))
}

fn resolve_exposure(value: &str) -> Option<McpExposure> {
    McpExposure::parse(value)
}

fn exposure_name(exposure: McpExposure) -> &'static str {
    match exposure {
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Direct => "direct",
        McpExposure::Hidden => "hidden",
    }
}

/// Validate one `mcpServers` entry.
pub fn validate_mcp_server_config(name: &str, raw: &Value) -> Result<McpServerConfig, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
    {
        return Err(format!(
            "invalid server name \"{name}\" (use letters, digits, \"_\" and \"-\")"
        ));
    }
    let Some(object) = raw.as_object() else {
        return Err(format!("server \"{name}\" must be an object"));
    };
    // Resolve exposure aliases before serde sees the enum values.
    let mut normalized = raw.clone();
    if let Some(object) = normalized.as_object_mut() {
        if let Some(exposure) = object.get("exposure").and_then(Value::as_str) {
            if let Some(exposure) = McpExposure::parse(exposure) {
                object.insert(
                    "exposure".to_string(),
                    Value::String(exposure_name(exposure).to_string()),
                );
            }
        }
        if let Some(tool_exposure) = object.get_mut("toolExposure").and_then(Value::as_object_mut) {
            for value in tool_exposure.values_mut() {
                if let Some(exposure) = value.as_str().and_then(McpExposure::parse) {
                    *value = Value::String(exposure_name(exposure).to_string());
                }
            }
        }
    }
    let mut config: McpServerConfig = serde_json::from_value(normalized)
        .map_err(|error| format!("server \"{name}\": {error}"))?;
    // Resolve exposure aliases before validating.
    if let Some(exposure) = object.get("exposure").and_then(Value::as_str) {
        config.exposure = Some(
            resolve_exposure(exposure)
                .ok_or_else(|| format!("server \"{name}\": exposure must be one of \"codemode\", \"deferred\", \"direct\", \"hidden\""))?,
        );
    }
    if let Some(tool_exposure) = object.get("toolExposure") {
        if !is_record(tool_exposure) {
            return Err(format!(
                "server \"{name}\": toolExposure must map tool names to exposures"
            ));
        }
        let Some(tool_exposure_object) = tool_exposure.as_object() else {
            return Err(format!(
                "server \"{name}\": toolExposure must map tool names to exposures"
            ));
        };
        let mut resolved = IndexMap::new();
        for (tool, exposure) in tool_exposure_object {
            let exposure = exposure
                .as_str()
                .and_then(resolve_exposure)
                .ok_or_else(|| format!("server \"{name}\": toolExposure \"{tool}\" must be one of \"codemode\", \"deferred\", \"direct\", \"hidden\""))?;
            resolved.insert(tool.clone(), exposure);
        }
        config.tool_exposure = resolved;
    }
    if let Some(enabled) = &config.enabled {
        let _ = enabled;
    } else if object.get("enabled").is_some_and(|value| !value.is_boolean()) {
        return Err(format!("server \"{name}\": enabled must be a boolean"));
    }
    if object
        .get("description")
        .is_some_and(|value| !value.is_string())
    {
        return Err(format!("server \"{name}\": description must be a string"));
    }
    if let Some(timeout) = config.timeout {
        if timeout <= 0.0 {
            return Err(format!(
                "server \"{name}\": timeout must be a positive number of seconds"
            ));
        }
    } else if object.get("timeout").is_some_and(|value| !value.is_number()) {
        return Err(format!(
            "server \"{name}\": timeout must be a positive number of seconds"
        ));
    }
    if config.kind.as_deref() == Some("sse")
        && config.command.is_some()
    {
        return Err(format!(
            "server \"{name}\": legacy SSE transport needs a url, not a command"
        ));
    }

    if let Some(url) = &config.url {
        let kind_allowed = matches!(
            config.kind.as_deref(),
            None | Some("http") | Some("streamable-http") | Some("sse")
        );
        if !kind_allowed {
            return Err(format!(
                "server \"{name}\": unsupported type \"{}\" for a url server",
                config.kind.as_deref().unwrap_or_default()
            ));
        }
        let parsed = Url::parse(url)
            .map_err(|_| format!("server \"{name}\": url must be an http or https URL"))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(format!(
                "server \"{name}\": url must be an http or https URL"
            ));
        }
        if let Some(headers) = object.get("headers") {
            if !is_string_record(headers) {
                return Err(format!(
                    "server \"{name}\": headers must map names to strings"
                ));
            }
        }
        config.oauth = validate_oauth(object.get("oauth"))
            .map_err(|error| format!("server \"{name}\": {error}"))?;
        config.auth = validate_auth(object.get("auth"), url)
            .map_err(|error| format!("server \"{name}\": {error}"))?;
        return Ok(config);
    }

    if let Some(command) = &config.command {
        if !matches!(config.kind.as_deref(), None | Some("stdio")) {
            return Err(format!(
                "server \"{name}\": unsupported type \"{}\" for a command server",
                config.kind.as_deref().unwrap_or_default()
            ));
        }
        let _ = command;
        if let Some(args) = object.get("args") {
            if !args
                .as_array()
                .is_some_and(|args| args.iter().all(Value::is_string))
            {
                return Err(format!("server \"{name}\": args must be an array of strings"));
            }
        }
        if let Some(env) = object.get("env") {
            if !is_string_record(env) {
                return Err(format!("server \"{name}\": env must map names to strings"));
            }
        }
        if object.get("cwd").is_some_and(|value| !value.is_string()) {
            return Err(format!("server \"{name}\": cwd must be a string"));
        }
        return Ok(config);
    }

    Err(format!(
        "server \"{name}\" needs either \"command\" (stdio) or \"url\" (streamable HTTP)"
    ))
}

/// Scope a server was defined in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpScope {
    /// `{agent_dir}/mcp.json`.
    Global,
    /// `{cwd}/.pi-rs/mcp.json`.
    Project,
    /// Registered by an extension (`pi.registerMcpServer`).
    Extension,
}

impl McpScope {
    /// Human-readable name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
            Self::Extension => "extension",
        }
    }
}

/// One configured server with its origin.
#[derive(Debug, Clone)]
pub struct LoadedServer {
    /// Server name.
    pub name: String,
    /// Parsed config.
    pub config: McpServerConfig,
    /// Config file that defined the entry.
    pub source: PathBuf,
    /// Global or project.
    pub scope: McpScope,
    /// Project `mcp.json` that overrides this global server, when any.
    pub override_path: Option<PathBuf>,
}

impl LoadedServer {
    /// Whether the server is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.is_enabled()
    }

    /// Exposure of the server.
    pub fn exposure(&self) -> McpExposure {
        self.config.exposure()
    }
}

/// A server registered by an extension.
#[derive(Debug, Clone)]
pub struct RegisteredMcpServer {
    /// Server name.
    pub name: String,
    /// Parsed config.
    pub config: McpServerConfig,
    /// Path of the extension that registered it.
    pub extension_path: String,
}

/// Servers registered by extensions of one runtime.
#[derive(Default)]
pub struct McpServerRegistry {
    servers: Mutex<HashMap<String, RegisteredMcpServer>>,
    change_listener: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl std::fmt::Debug for McpServerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerRegistry")
            .field("servers", &self.list().len())
            .finish()
    }
}

impl McpServerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    fn servers(&self) -> MutexGuard<'_, HashMap<String, RegisteredMcpServer>> {
        self.servers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Register or replace a server.
    pub fn register(&self, server: RegisteredMcpServer) {
        self.servers().insert(server.name.clone(), server);
        self.notify();
    }

    /// Remove a server registered by `extension_path`.
    pub fn unregister(&self, name: &str, extension_path: &str) {
        let mut servers = self.servers();
        if servers
            .get(name)
            .is_some_and(|server| server.extension_path == extension_path)
        {
            servers.remove(name);
            drop(servers);
            self.notify();
        }
    }

    /// Get a server by name.
    pub fn get(&self, name: &str) -> Option<RegisteredMcpServer> {
        self.servers().get(name).cloned()
    }

    /// All registered servers, in registration order.
    pub fn list(&self) -> Vec<RegisteredMcpServer> {
        self.servers().values().cloned().collect()
    }

    /// Set the change listener.
    pub fn set_change_listener(&self, listener: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self
            .change_listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = listener;
    }

    fn notify(&self) {
        let listener = self
            .change_listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(listener) = listener {
            listener();
        }
    }
}

/// Characters of a server description in the system prompt section.
const MAX_SERVER_DESCRIPTION_CHARS: usize = 250;
/// Characters of the whole `mcp_servers` section.
pub const MAX_SERVERS_SECTION_CHARS: usize = 4096;

fn truncate(text: &str, max: usize) -> String {
    // v1.5.0 measures UTF-16 code units (`text.length`) and uses
    // `sliceHead(text, max - 1)`.
    if pi_ai::utils::text::utf16_len(text) <= max {
        return text.to_string();
    }
    if max <= 1 {
        return String::new();
    }
    let mut truncated = pi_ai::utils::text::truncate_utf16(text, max - 1).to_string();
    truncated = truncated.trim_end().to_string();
    truncated.push('…');
    truncated
}

/// Render the `mcp_servers` system prompt section.
///
/// Only enabled servers whose tools are not declared directly are listed;
/// pi-rs reaches both `codemode` and `deferred` through `tool_search`.
pub fn render_mcp_servers_section(
    listings: &[(String, McpExposure, String)],
) -> Option<String> {
    let mut listed: Vec<&(String, McpExposure, String)> = listings
        .iter()
        .filter(|(_, exposure, _)| {
            matches!(exposure, McpExposure::Codemode | McpExposure::Deferred)
        })
        .collect();
    if listed.is_empty() {
        return None;
    }
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    let intro = "MCP servers whose tools are not declared to you. Load their tools with `tool_search`.";
    let heads: Vec<String> = listed
        .iter()
        .map(|(name, _, _)| format!("- {} (tool_search)", mcp_namespace(name)))
        .collect();
    let size = |kept: usize| -> usize {
        let omitted = listed.len().saturating_sub(kept);
        let mut length = pi_ai::utils::text::utf16_len(intro) + 1;
        for head in heads.iter().take(kept) {
            length += pi_ai::utils::text::utf16_len(head) + 1;
        }
        if omitted > 0 {
            length += pi_ai::utils::text::utf16_len(&format!(
                "- … {omitted} more server{}; find their tools with `tool_search`",
                if omitted == 1 { "" } else { "s" }
            )) + 1;
        }
        length
    };
    let mut kept = listed.len();
    while kept > 0 && size(kept) > MAX_SERVERS_SECTION_CHARS {
        kept -= 1;
    }
    let per_server = if kept == 0 {
        0
    } else {
        let remaining = MAX_SERVERS_SECTION_CHARS.saturating_sub(size(kept));
        ((remaining / kept).saturating_sub(2)).min(MAX_SERVER_DESCRIPTION_CHARS)
    };
    let mut lines = vec![intro.to_string()];
    for (index, (_, _, summary)) in listed.iter().take(kept).enumerate() {
        let summary = if per_server > 0 {
            truncate(summary, per_server)
        } else {
            String::new()
        };
        if summary.is_empty() {
            lines.push(heads[index].clone());
        } else {
            lines.push(format!("{}: {summary}", heads[index]));
        }
    }
    if kept < listed.len() {
        let omitted = listed.len() - kept;
        lines.push(format!(
            "- … {omitted} more server{}; find their tools with `tool_search`",
            if omitted == 1 { "" } else { "s" }
        ));
    }
    Some(lines.join("\n"))
}

/// Path of the global config file.
pub fn global_config_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("mcp.json")
}

/// Path of the project config file.
pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(CONFIG_DIR_NAME).join("mcp.json")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_stdio_and_http() {
        let stdio = validate_mcp_server_config(
            "fs",
            &json!({"command": "npx", "args": ["-y", "server"], "env": {"A": "b"}}),
        )
        .unwrap();
        assert!(stdio.is_stdio());
        assert_eq!(stdio.exposure(), McpExposure::Codemode);

        let http = validate_mcp_server_config(
            "docs",
            &json!({"url": "https://example.com/mcp", "exposure": "direct"}),
        )
        .unwrap();
        assert!(http.is_http());
        assert_eq!(http.exposure(), McpExposure::Direct);
    }

    #[test]
    fn exposure_alias_and_tool_overrides() {
        let config = validate_mcp_server_config(
            "x",
            &json!({
                "command": "a",
                "exposure": "codemode-deferred",
                "toolExposure": {"read_*": "direct", "write": "hidden"}
            }),
        )
        .unwrap();
        assert_eq!(config.exposure(), McpExposure::Codemode);
        assert_eq!(config.tool_exposure("read_file"), McpExposure::Direct);
        assert_eq!(config.tool_exposure("write"), McpExposure::Hidden);
        assert_eq!(config.tool_exposure("other"), McpExposure::Codemode);
    }

    #[test]
    fn rejects_invalid_names_and_missing_transport() {
        assert!(validate_mcp_server_config("bad name", &json!({"command": "a"})).is_err());
        assert!(validate_mcp_server_config("x", &json!({})).is_err());
    }

    #[test]
    fn validates_oauth_callback_url() {
        let error = validate_mcp_server_config(
            "x",
            &json!({"url": "https://example.com/mcp", "oauth": {"callbackUrl": "https://example.com/cb"}}),
        )
        .unwrap_err();
        assert!(error.contains("callbackUrl"));
        let config = validate_mcp_server_config(
            "x",
            &json!({"url": "https://example.com/mcp", "oauth": {"callbackUrl": "http://127.0.0.1:7777/callback"}}),
        )
        .unwrap();
        assert_eq!(
            config.oauth.unwrap().callback_port,
            None,
        );
    }

    #[test]
    fn namespace_replaces_dashes() {
        assert_eq!(mcp_namespace("my-server"), "mcp__my_server");
    }

    #[test]
    fn registry_removes_only_owning_extension() {
        let registry = McpServerRegistry::new();
        let config = validate_mcp_server_config("x", &json!({"command": "a"})).unwrap();
        registry.register(RegisteredMcpServer {
            name: "x".to_string(),
            config,
            extension_path: "ext-a".to_string(),
        });
        registry.unregister("x", "ext-b");
        assert!(registry.get("x").is_some());
        registry.unregister("x", "ext-a");
        assert!(registry.get("x").is_none());
    }

    #[test]
    fn renders_only_indirect_servers() {
        let section = render_mcp_servers_section(&[
            (
                "docs".to_string(),
                McpExposure::Codemode,
                "Documentation search".to_string(),
            ),
            (
                "direct".to_string(),
                McpExposure::Direct,
                "Should not appear".to_string(),
            ),
            (
                "tools".to_string(),
                McpExposure::Deferred,
                "Deferred tools".to_string(),
            ),
        ])
        .unwrap();
        assert!(section.contains("mcp__docs"));
        assert!(section.contains("mcp__tools"));
        assert!(!section.contains("mcp__direct"));
        assert!(section.contains("tool_search"));
    }
}
