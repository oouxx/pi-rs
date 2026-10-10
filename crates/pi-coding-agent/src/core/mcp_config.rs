//! MCP server configuration files.
//!
//! Servers come from `mcp.json` in the agent directory (global) and
//! `{cwd}/.pi-rs/mcp.json` (project). Both use the common `mcpServers` shape,
//! so existing MCP client configs can be copied over. A project entry replaces
//! a global entry with the same name.
//!
//! pi treats every project as trusted (see `DEVIATIONS.md`), so the project
//! file is always loaded.
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "fs":   { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] },
//!     "docs": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" } },
//!     "legacy": { "type": "sse", "url": "https://example.com/sse" }
//!   }
//! }
//! ```
//!
//! This module is feature-gated behind `mcp` (default-on).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::CONFIG_DIR_NAME;
use crate::core::mcp::{McpServerSpec, McpTransportSpec};
use crate::core::resolve_config_value::resolve_config_value_or_throw;

/// Global config file path (`{agent_dir}/mcp.json`).
pub fn global_config_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("mcp.json")
}

/// Project config file path (`{cwd}/.pi-rs/mcp.json`).
pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(CONFIG_DIR_NAME).join("mcp.json")
}

/// Scope a server was defined in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpScope {
    /// `{agent_dir}/mcp.json`.
    Global,
    /// `{cwd}/.pi-rs/mcp.json`.
    Project,
}

impl McpScope {
    /// Human-readable name used in `pi-rs mcp list` / errors.
    pub fn as_str(&self) -> &'static str {
        match self {
            McpScope::Global => "global",
            McpScope::Project => "project",
        }
    }
}

/// One configured server with its origin.
#[derive(Debug, Clone)]
pub struct LoadedServer {
    /// Server name (the `mcpServers` key).
    pub name: String,
    /// Resolved transport.
    pub transport: McpTransportSpec,
    /// Whether the entry is enabled (`enabled: false` disables it).
    pub enabled: bool,
    /// Config file the entry came from.
    pub source: PathBuf,
    /// Global or project.
    pub scope: McpScope,
}

impl LoadedServer {
    /// Build a connectable spec (ignores `enabled`; callers filter).
    pub fn spec(&self) -> McpServerSpec {
        McpServerSpec {
            name: self.name.clone(),
            transport: self.transport.clone(),
        }
    }
}

/// Raw `mcp.json` file.
#[derive(Debug, Default, Deserialize)]
struct RawFile {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, RawServer>,
}

/// One raw `mcpServers` entry. `command` (stdio) and `url` (HTTP/SSE) are
/// mutually exclusive; `type` disambiguates `url` between HTTP and SSE.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct RawServer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    headers: Option<BTreeMap<String, String>>,
}

fn read_file(path: &Path, scope: McpScope, errors: &mut Vec<String>) -> Vec<(String, RawServer)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let parsed: RawFile = match serde_json::from_str(&text) {
        Ok(parsed) => parsed,
        Err(e) => {
            errors.push(format!("{}: {e}", path.display()));
            return Vec::new();
        }
    };
    let _ = scope;
    parsed.mcp_servers.into_iter().collect()
}

/// Turn one raw entry into a transport, resolving `${VAR}` / `!cmd` references
/// in env values and headers. Returns an error message on invalid config.
fn resolve_transport(name: &str, raw: &RawServer) -> Result<McpTransportSpec, String> {
    let kind = raw.kind.as_deref().unwrap_or("");

    if let Some(url) = &raw.url {
        let headers = resolve_map(raw.headers.as_ref(), name, "header")?;
        return match kind {
            "sse" => Ok(McpTransportSpec::Sse {
                url: url.clone(),
                headers,
            }),
            "" | "http" | "streamable-http" => Ok(McpTransportSpec::Http {
                url: url.clone(),
                headers,
            }),
            other => Err(format!(
                "server \"{name}\": unsupported type \"{other}\" for a url server"
            )),
        };
    }

    if let Some(command) = &raw.command {
        if !kind.is_empty() && kind != "stdio" {
            return Err(format!(
                "server \"{name}\": unsupported type \"{kind}\" for a command server"
            ));
        }
        let env = resolve_map(raw.env.as_ref(), name, "env")?;
        return Ok(McpTransportSpec::Stdio {
            command: command.clone(),
            args: raw.args.clone().unwrap_or_default(),
            env,
            cwd: raw.cwd.clone(),
        });
    }

    Err(format!(
        "server \"{name}\" needs either \"command\" (stdio) or \"url\" (HTTP/SSE)"
    ))
}

fn resolve_map(
    map: Option<&BTreeMap<String, String>>,
    name: &str,
    what: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut resolved = BTreeMap::new();
    for (key, value) in map.iter().flat_map(|m| m.iter()) {
        let value = resolve_config_value_or_throw(value, &format!("MCP server \"{name}\" {what} \"{key}\""))
            .map_err(|e| format!("server \"{name}\": {e}"))?;
        resolved.insert(key.clone(), value);
    }
    Ok(resolved)
}

/// Load global + project servers. Project entries replace global entries with
/// the same name. Invalid entries are reported in the error list and skipped.
pub fn load_servers(cwd: &str, agent_dir: &str) -> (Vec<LoadedServer>, Vec<String>) {
    let mut errors = Vec::new();
    let mut by_name: BTreeMap<String, LoadedServer> = BTreeMap::new();

    let global_path = global_config_path(Path::new(agent_dir));
    for (name, raw) in read_file(&global_path, McpScope::Global, &mut errors) {
        match resolve_transport(&name, &raw) {
            Ok(transport) => {
                by_name.insert(
                    name.clone(),
                    LoadedServer {
                        name,
                        transport,
                        enabled: raw.enabled.unwrap_or(true),
                        source: global_path.clone(),
                        scope: McpScope::Global,
                    },
                );
            }
            Err(e) => errors.push(format!("{}: {e}", global_path.display())),
        }
    }

    let project_path = project_config_path(Path::new(cwd));
    for (name, raw) in read_file(&project_path, McpScope::Project, &mut errors) {
        match resolve_transport(&name, &raw) {
            Ok(transport) => {
                by_name.insert(
                    name.clone(),
                    LoadedServer {
                        name,
                        transport,
                        enabled: raw.enabled.unwrap_or(true),
                        source: project_path.clone(),
                        scope: McpScope::Project,
                    },
                );
            }
            Err(e) => errors.push(format!("{}: {e}", project_path.display())),
        }
    }

    (by_name.into_values().collect(), errors)
}

/// Build specs for every enabled server.
pub fn load_specs(cwd: &str, agent_dir: &str) -> (Vec<McpServerSpec>, Vec<String>) {
    let (servers, errors) = load_servers(cwd, agent_dir);
    let specs = servers
        .into_iter()
        .filter(|s| s.enabled)
        .map(|s| s.spec())
        .collect();
    (specs, errors)
}

// ============================================================================
// Editing (used by `pi-rs mcp add/remove/enable/disable`)
// ============================================================================

fn read_value(path: &Path) -> Result<serde_json::Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write_value(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, format!("{text}\n")).map_err(|e| format!("{}: {e}", path.display()))
}

fn servers_object_mut(
    value: &mut serde_json::Value,
) -> Result<&mut serde_json::Map<String, serde_json::Value>, String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "mcp.json must contain a JSON object".to_string())?;
    if !object.contains_key("mcpServers") {
        object.insert("mcpServers".to_string(), serde_json::json!({}));
    }
    object
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| "mcp.json: \"mcpServers\" must be an object".to_string())
}

/// Add or replace a server entry. Returns `true` if an existing entry was
/// replaced.
pub fn add_server(path: &Path, name: &str, raw: &serde_json::Value) -> Result<bool, String> {
    let mut value = read_value(path)?;
    let servers = servers_object_mut(&mut value)?;
    let replaced = servers.contains_key(name);
    servers.insert(name.to_string(), raw.clone());
    write_value(path, &value)?;
    Ok(replaced)
}

/// Remove a server entry. Returns `true` if it existed.
pub fn remove_server(path: &Path, name: &str) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let mut value = read_value(path)?;
    let servers = servers_object_mut(&mut value)?;
    let removed = servers.remove(name).is_some();
    if removed {
        write_value(path, &value)?;
    }
    Ok(removed)
}

/// Enable or disable a server entry (writes/removes `enabled`). Returns `false`
/// when the file does not define the server.
pub fn set_enabled(path: &Path, name: &str, enabled: bool) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let mut value = read_value(path)?;
    let servers = servers_object_mut(&mut value)?;
    let Some(entry) = servers.get_mut(name).and_then(|v| v.as_object_mut()) else {
        return Ok(false);
    };
    if enabled {
        entry.remove("enabled");
    } else {
        entry.insert("enabled".to_string(), serde_json::Value::Bool(false));
    }
    write_value(path, &value)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn loads_global_and_project_with_project_override() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("proj");
        write(
            &agent_dir.join("mcp.json"),
            r#"{"mcpServers":{"fs":{"command":"npx","args":["a"]},"old":{"command":"x"}}}"#,
        );
        write(
            &project_config_path(&cwd),
            r#"{"mcpServers":{"fs":{"url":"https://example.com/mcp"},"proj":{"command":"y"}}}"#,
        );

        let (servers, errors) = load_servers(
            cwd.to_str().unwrap(),
            agent_dir.to_str().unwrap(),
        );
        assert!(errors.is_empty(), "{errors:?}");
        let names: Vec<_> = servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["fs", "old", "proj"]);
        let fs = servers.iter().find(|s| s.name == "fs").unwrap();
        assert_eq!(fs.scope, McpScope::Project);
        assert!(matches!(fs.transport, McpTransportSpec::Http { .. }));
    }

    #[test]
    fn disabled_servers_are_excluded_from_specs() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        write(
            &agent_dir.join("mcp.json"),
            r#"{"mcpServers":{"on":{"command":"a"},"off":{"command":"b","enabled":false}}}"#,
        );
        let (specs, errors) =
            load_specs(dir.path().to_str().unwrap(), agent_dir.to_str().unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "on");
    }

    #[test]
    fn sse_type_maps_to_sse_transport() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        write(
            &agent_dir.join("mcp.json"),
            r#"{"mcpServers":{"l":{"type":"sse","url":"https://example.com/sse"}}}"#,
        );
        let (specs, errors) =
            load_specs(dir.path().to_str().unwrap(), agent_dir.to_str().unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        assert!(matches!(specs[0].transport, McpTransportSpec::Sse { .. }));
    }

    #[test]
    fn invalid_entry_is_reported_and_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        write(
            &agent_dir.join("mcp.json"),
            r#"{"mcpServers":{"bad":{"type":"sse","command":"x"},"good":{"command":"a"}}}"#,
        );
        let (specs, errors) =
            load_specs(dir.path().to_str().unwrap(), agent_dir.to_str().unwrap());
        assert_eq!(errors.len(), 1);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "good");
    }

    #[test]
    fn add_remove_enable_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let raw = serde_json::json!({"command": "npx", "args": ["-y", "server"]});
        assert!(!add_server(&path, "fs", &raw).unwrap());
        assert!(add_server(&path, "fs", &raw).unwrap());
        assert!(set_enabled(&path, "fs", false).unwrap());
        let (servers, errors) =
            load_servers(dir.path().to_str().unwrap(), dir.path().to_str().unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        // Global path is agent_dir/mcp.json, and here agent_dir == cwd == dir,
        // so the file is read as the global config.
        assert_eq!(servers.len(), 1);
        assert!(!servers[0].enabled);
        assert!(remove_server(&path, "fs").unwrap());
        assert!(!remove_server(&path, "fs").unwrap());
    }
}
