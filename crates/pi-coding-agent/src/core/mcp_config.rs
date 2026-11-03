//! MCP server configuration files.
//!
//! Port of `packages/coding-agent/src/extensions/mcp/config.ts`.
//!
//! Servers come from `{agent_dir}/mcp.json` (global) and
//! `{cwd}/.pi-rs/mcp.json` (project). pi-rs treats every project as trusted
//! (see `DEVIATIONS.md`), so the project file is always loaded. A project
//! entry without `command`/`url`/`type` overrides only `enabled`, `exposure`,
//! and `toolExposure` of the global server with the same name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::core::mcp_servers::{
    global_config_path, project_config_path, LoadedServer, McpExposure, McpScope,
    McpServerConfig,
};
use crate::core::mcp_servers::{mcp_namespace, validate_mcp_server_config};

const OVERRIDE_KEYS: [&str; 3] = ["enabled", "exposure", "toolExposure"];

/// Result of loading global and project configuration.
#[derive(Debug, Clone, Default)]
pub struct LoadedMcpConfig {
    /// Servers in merge order.
    pub servers: Vec<LoadedServer>,
    /// Whether connecting `codemode` servers should activate codemode.
    pub auto_enable_codemode: Option<bool>,
    /// Config errors.
    pub errors: Vec<String>,
    /// Project `mcp.json`, where `/mcp` saves project overrides.
    pub project_config: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct RawFile {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, Value>,
    #[serde(default, rename = "autoEnableCodemode")]
    auto_enable_codemode: Option<Value>,
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

fn is_override(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| {
            object.get("command").is_none()
                && object.get("url").is_none()
                && object.get("type").is_none()
        })
}

fn read_config_file(
    path: &Path,
    scope: McpScope,
    servers: &mut BTreeMap<String, LoadedServer>,
    auto_enable_codemode: &mut Option<bool>,
    errors: &mut Vec<String>,
) {
    if !path.exists() {
        return;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let parsed: RawFile = match serde_json::from_str(text) {
        Ok(parsed) => parsed,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    match &parsed.auto_enable_codemode {
        Some(Value::Bool(value)) => *auto_enable_codemode = Some(*value),
        Some(_) => errors.push(format!(
            "{}: autoEnableCodemode must be a boolean",
            path.display()
        )),
        None => {}
    }

    for (name, value) in parsed.mcp_servers {
        if scope == McpScope::Project && is_record(&value) && is_override(&value) {
            let Some(base) = servers.get(&name).cloned() else {
                errors.push(format!(
                    "{}: server \"{name}\" needs \"command\" or \"url\", or a global server to override",
                    path.display()
                ));
                continue;
            };
            let extra: Vec<String> = value
                .as_object()
                .map(|object| {
                    object
                        .keys()
                        .filter(|key| !OVERRIDE_KEYS.contains(&key.as_str()))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if !extra.is_empty() {
                errors.push(format!(
                    "{}: server \"{name}\": an override can only set {}",
                    path.display(),
                    OVERRIDE_KEYS.join(", ")
                ));
                continue;
            }
            let mut merged = serde_json::to_value(&base.config).unwrap_or(Value::Null);
            if let (Some(merged), Some(override_values)) = (merged.as_object_mut(), value.as_object()) {
                for key in OVERRIDE_KEYS {
                    if let Some(override_value) = override_values.get(key) {
                        merged.insert(key.to_string(), override_value.clone());
                    }
                }
            }
            match validate_mcp_server_config(&name, &merged) {
                Ok(config) => {
                    servers.insert(
                        name.clone(),
                        LoadedServer {
                            name,
                            config,
                            source: base.source,
                            scope: base.scope,
                            override_path: Some(path.to_path_buf()),
                        },
                    );
                }
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
            continue;
        }

        let config = match validate_mcp_server_config(&name, &value) {
            Ok(config) => config,
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        // Names that differ only in `-` and `_` would share a namespace.
        if let Some(clash) = servers.keys().find(|other| {
            other.as_str() != name.as_str()
                && mcp_namespace(other) == mcp_namespace(&name)
        }) {
            errors.push(format!(
                "{}: server \"{name}\" conflicts with \"{clash}\"",
                path.display()
            ));
            continue;
        }
        if scope == McpScope::Project && config.auth.is_some() {
            errors.push(format!(
                "{}: server \"{name}\": auth is only allowed in the global mcp.json",
                path.display()
            ));
            continue;
        }
        servers.insert(
            name.clone(),
            LoadedServer {
                name,
                config,
                source: path.to_path_buf(),
                scope,
                override_path: None,
            },
        );
    }
}

/// Load global and project MCP configuration.
pub fn load_mcp_config(cwd: &str, agent_dir: &str) -> LoadedMcpConfig {
    let mut servers = BTreeMap::new();
    let mut auto_enable_codemode = None;
    let mut errors = Vec::new();
    read_config_file(
        &global_config_path(Path::new(agent_dir)),
        McpScope::Global,
        &mut servers,
        &mut auto_enable_codemode,
        &mut errors,
    );
    let project_config = project_config_path(Path::new(cwd));
    read_config_file(
        &project_config,
        McpScope::Project,
        &mut servers,
        &mut auto_enable_codemode,
        &mut errors,
    );
    LoadedMcpConfig {
        servers: servers.into_values().collect(),
        auto_enable_codemode,
        errors,
        project_config: Some(project_config),
    }
}

/// Compatibility wrapper returning servers and errors.
pub fn load_servers(cwd: &str, agent_dir: &str) -> (Vec<LoadedServer>, Vec<String>) {
    let loaded = load_mcp_config(cwd, agent_dir);
    (loaded.servers, loaded.errors)
}

/// Patch applied by `/mcp` and `pi-rs mcp enable/disable`.
#[derive(Debug, Clone, Default)]
pub struct McpServerConfigPatch {
    /// Whether the server is enabled.
    pub enabled: Option<bool>,
    /// Default exposure.
    pub exposure: Option<McpExposure>,
}

fn read_value(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            serde_json::from_str(text).map_err(|error| format!("{}: {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn write_value(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let indent = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with(' ') || line.starts_with('\t'))
                .map(|line| {
                    line.chars()
                        .take_while(|character| *character == ' ' || *character == '\t')
                        .collect::<String>()
                })
        })
        .unwrap_or_else(|| "  ".to_string());
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    value
        .serialize(&mut serializer)
        .map_err(|error| error.to_string())?;
    let mut text = String::from_utf8(buffer).map_err(|error| error.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

fn servers_object_mut(
    value: &mut Value,
) -> Result<&mut serde_json::Map<String, Value>, String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "mcp.json must contain a JSON object".to_string())?;
    if !object.contains_key("mcpServers") {
        object.insert("mcpServers".to_string(), serde_json::json!({}));
    }
    object
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "mcp.json: \"mcpServers\" must be an object".to_string())
}

/// Add or replace a server entry. Returns true when an entry was replaced.
pub fn add_server(path: &Path, name: &str, raw: &Value) -> Result<bool, String> {
    validate_mcp_server_config(name, raw)?;
    let mut value = read_value(path)?;
    let servers = servers_object_mut(&mut value)?;
    let replaced = servers.contains_key(name);
    servers.insert(name.to_string(), raw.clone());
    write_value(path, &value)?;
    Ok(replaced)
}

/// Remove a server entry. Returns true when it existed.
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

/// Change one server's settings.
pub fn update_server(
    path: &Path,
    name: &str,
    patch: &McpServerConfigPatch,
    allow_override: bool,
) -> Result<bool, String> {
    let mut value = read_value(path)?;
    let servers = servers_object_mut(&mut value)?;
    let entry = servers.get_mut(name);
    let entry = match entry {
        Some(entry) => entry,
        None if allow_override => {
            servers.insert(name.to_string(), serde_json::json!({}));
            servers
                .get_mut(name)
                .ok_or_else(|| format!("{}: could not add \"{name}\"", path.display()))?
        }
        None => return Ok(false),
    };
    let object = entry
        .as_object_mut()
        .ok_or_else(|| format!("{} does not define MCP server \"{name}\"", path.display()))?;
    let keep_defaults = object.get("command").is_none()
        && object.get("url").is_none()
        && object.get("type").is_none();
    if let Some(enabled) = patch.enabled {
        if enabled && !keep_defaults {
            object.remove("enabled");
        } else {
            object.insert("enabled".to_string(), Value::Bool(enabled));
        }
    }
    if let Some(exposure) = patch.exposure {
        if exposure == McpExposure::Codemode && !keep_defaults {
            object.remove("exposure");
        } else {
            let name = match exposure {
                McpExposure::Codemode => "codemode",
                McpExposure::Deferred => "deferred",
                McpExposure::Direct => "direct",
                McpExposure::Hidden => "hidden",
            };
            object.insert("exposure".to_string(), Value::String(name.to_string()));
        }
    }
    write_value(path, &value)?;
    Ok(true)
}

/// Enable or disable a server entry. Returns false when the file does not
/// define the server.
pub fn set_enabled(path: &Path, name: &str, enabled: bool) -> Result<bool, String> {
    update_server(
        path,
        name,
        &McpServerConfigPatch {
            enabled: Some(enabled),
            exposure: None,
        },
        false,
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::core::mcp_servers::project_config_path;
    use serde_json::json;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn loads_global_and_project_with_override() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("proj");
        write(
            &agent_dir.join("mcp.json"),
            r#"{"mcpServers":{"fs":{"command":"npx","args":["a"]},"old":{"command":"x"}}}"#,
        );
        write(
            &project_config_path(&cwd),
            r#"{"mcpServers":{"fs":{"enabled":false,"exposure":"direct"},"proj":{"command":"y"}}}"#,
        );

        let loaded = load_mcp_config(cwd.to_str().unwrap(), agent_dir.to_str().unwrap());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let names: Vec<_> = loaded.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["fs", "old", "proj"]);
        let fs = loaded.servers.iter().find(|s| s.name == "fs").unwrap();
        assert_eq!(fs.scope, McpScope::Global);
        assert!(!fs.config.is_enabled());
        assert_eq!(fs.config.exposure(), McpExposure::Direct);
        assert_eq!(fs.override_path, Some(project_config_path(&cwd)));
    }

    #[test]
    fn project_cannot_set_auth() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("proj");
        write(
            &project_config_path(&cwd),
            r#"{"mcpServers":{"remote":{"url":"https://example.com/mcp","auth":{"provider":"openai"}}}}"#,
        );
        let loaded = load_mcp_config(cwd.to_str().unwrap(), agent_dir.to_str().unwrap());
        assert!(loaded.servers.is_empty());
        assert!(
            loaded.errors.iter().any(|error| error.contains("auth is only allowed")),
            "{:?}",
            loaded.errors
        );
    }

    #[test]
    fn namespace_conflict_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        write(
            &agent_dir.join("mcp.json"),
            r#"{"mcpServers":{"a-b":{"command":"x"},"a_b":{"command":"y"}}}"#,
        );
        let loaded = load_mcp_config(dir.path().to_str().unwrap(), agent_dir.to_str().unwrap());
        assert_eq!(loaded.servers.len(), 1);
        assert!(
            loaded.errors.iter().any(|error| error.contains("conflicts with")),
            "{:?}",
            loaded.errors
        );
    }

    #[test]
    fn add_remove_enable_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let raw = json!({"command": "npx", "args": ["-y", "server"]});
        assert!(!add_server(&path, "fs", &raw).unwrap());
        assert!(add_server(&path, "fs", &raw).unwrap());
        assert!(set_enabled(&path, "fs", false).unwrap());
        let (servers, errors) =
            load_servers(dir.path().to_str().unwrap(), dir.path().to_str().unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(servers.len(), 1);
        assert!(!servers[0].config.is_enabled());
        assert!(remove_server(&path, "fs").unwrap());
        assert!(!remove_server(&path, "fs").unwrap());
    }
}
