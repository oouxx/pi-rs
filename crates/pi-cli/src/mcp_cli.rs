//! `pi-rs mcp` — manage the MCP servers in `mcp.json`.
//!
//! Config-only (no live connection): `list` prints the merged global +
//! project configuration, `add`/`remove`/`enable`/`disable` edit the file that
//! defines a server (project by default for `add`; `--global` targets the
//! agent-dir file).
//!
//! Commands:
//! - `mcp list`
//! - `mcp add <name> -- <command> [args...]`
//! - `mcp add <name> --url <url> [--header Name:Value]... [--sse]`
//! - `mcp remove <name>`
//! - `mcp enable <name>` / `mcp disable <name>`

use std::path::Path;

use colored::*;
use pi_coding_agent::config;
use pi_coding_agent::core::mcp::McpTransportSpec;
use pi_coding_agent::core::mcp_config::{
    self, global_config_path, project_config_path, LoadedServer,
};

const USAGE: &str = "\
Usage: pi-rs mcp <command>

Commands:
  list                                   List configured MCP servers
  add <name> -- <command> [args...]      Add a stdio server (project mcp.json)
  add <name> --url <url> [--header N:V]  Add an HTTP server (add --sse for legacy SSE)
  remove <name>                          Remove a server from the file that defines it
  enable <name>                          Enable a server
  disable <name>                         Disable a server

Options:
  --global   Target the global {agent_dir}/mcp.json instead of the project file";

/// Handle `pi-rs mcp ...`. Returns a process exit code.
pub fn handle_mcp_command(args: &[String]) -> i32 {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/tmp".to_string());
    let agent_dir = config::get_agent_dir().to_string_lossy().to_string();

    let Some(command) = args.first().map(String::as_str) else {
        println!("{USAGE}");
        return 0;
    };

    match command {
        "list" => cmd_list(&cwd, &agent_dir),
        "add" => cmd_add(&args[1..], &cwd, &agent_dir),
        "remove" => cmd_edit(&args[1..], &cwd, &agent_dir, Edit::Remove),
        "enable" => cmd_edit(&args[1..], &cwd, &agent_dir, Edit::SetEnabled(true)),
        "disable" => cmd_edit(&args[1..], &cwd, &agent_dir, Edit::SetEnabled(false)),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            0
        }
        other => {
            eprintln!("{} Unknown mcp command: {other}", "Error:".red().bold());
            println!("{USAGE}");
            1
        }
    }
}

fn describe_transport(transport: &McpTransportSpec) -> String {
    match transport {
        McpTransportSpec::Stdio { command, args, .. } => {
            let mut parts = vec![command.clone()];
            parts.extend(args.iter().cloned());
            parts.join(" ")
        }
        McpTransportSpec::Http { url, .. } => format!("http {url}"),
        McpTransportSpec::Sse { url, .. } => format!("sse {url}"),
    }
}

fn cmd_list(cwd: &str, agent_dir: &str) -> i32 {
    let (servers, errors) = mcp_config::load_servers(cwd, agent_dir);
    if servers.is_empty() && errors.is_empty() {
        println!(
            "No MCP servers configured. Add them to {} or {}",
            global_config_path(Path::new(agent_dir)).display(),
            project_config_path(Path::new(cwd)).display()
        );
        return 0;
    }
    for server in &servers {
        let state = if server.enabled {
            "enabled".green().to_string()
        } else {
            "disabled".dimmed().to_string()
        };
        println!(
            "{:<16} {:<8} {:<7} {}",
            server.name,
            server.scope.as_str(),
            state,
            describe_transport(&server.transport)
        );
    }
    for error in &errors {
        eprintln!("{} {error}", "config error:".red().bold());
    }
    0
}

fn target_path(global: bool, cwd: &str, agent_dir: &str) -> std::path::PathBuf {
    if global {
        global_config_path(Path::new(agent_dir))
    } else {
        project_config_path(Path::new(cwd))
    }
}

fn cmd_add(args: &[String], cwd: &str, agent_dir: &str) -> i32 {
    let mut global = false;
    let mut url: Option<String> = None;
    let mut sse = false;
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut name: Option<String> = None;
    let mut command: Vec<String> = Vec::new();
    let mut after_separator = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if !after_separator && arg == "--global" {
            global = true;
        } else if !after_separator && arg == "--url" {
            i += 1;
            match args.get(i) {
                Some(value) => url = Some(value.clone()),
                None => {
                    eprintln!("{} --url needs a value", "Error:".red().bold());
                    return 1;
                }
            }
        } else if !after_separator && arg == "--header" {
            i += 1;
            match args.get(i).and_then(|h| h.split_once(':')) {
                Some((k, v)) => headers.push((k.trim().to_string(), v.trim().to_string())),
                None => {
                    eprintln!("{} --header needs Name:Value", "Error:".red().bold());
                    return 1;
                }
            }
        } else if !after_separator && arg == "--sse" {
            sse = true;
        } else if !after_separator && arg == "--" {
            after_separator = true;
        } else if name.is_none() {
            name = Some(arg.clone());
        } else {
            command.push(arg.clone());
        }
        i += 1;
    }

    let Some(name) = name else {
        eprintln!("{} mcp add needs a server name", "Error:".red().bold());
        return 1;
    };

    let entry = match url {
        Some(url) => {
            let mut map = serde_json::Map::new();
            if sse {
                map.insert("type".to_string(), serde_json::json!("sse"));
            }
            map.insert("url".to_string(), serde_json::json!(url));
            if !headers.is_empty() {
                let headers: serde_json::Map<String, serde_json::Value> = headers
                    .into_iter()
                    .map(|(k, v)| (k, serde_json::json!(v)))
                    .collect();
                map.insert(
                    "headers".to_string(),
                    serde_json::Value::Object(headers),
                );
            }
            serde_json::Value::Object(map)
        }
        None => {
            if command.is_empty() {
                eprintln!(
                    "{} mcp add needs a command (`-- <command> [args...]`) or --url <url>",
                    "Error:".red().bold()
                );
                return 1;
            }
            let mut map = serde_json::Map::new();
            map.insert("command".to_string(), serde_json::json!(command[0]));
            if command.len() > 1 {
                map.insert("args".to_string(), serde_json::json!(command[1..]));
            }
            serde_json::Value::Object(map)
        }
    };

    let path = target_path(global, cwd, agent_dir);
    match mcp_config::add_server(&path, &name, &entry) {
        Ok(replaced) => {
            let verb = if replaced { "Updated" } else { "Added" };
            println!("{verb} MCP server \"{name}\" in {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("{} {e}", "Error:".red().bold());
            1
        }
    }
}

enum Edit {
    Remove,
    SetEnabled(bool),
}

/// Find the file that defines `name` and edit it there.
fn cmd_edit(args: &[String], cwd: &str, agent_dir: &str, edit: Edit) -> i32 {
    let global = args.iter().any(|a| a == "--global");
    let Some(name) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("{} this command needs a server name", "Error:".red().bold());
        return 1;
    };

    let path = if global {
        target_path(true, cwd, agent_dir)
    } else {
        // Prefer the file that defines the server so edits land in the right
        // place; `--global` forces the agent-dir file.
        let (servers, _) = mcp_config::load_servers(cwd, agent_dir);
        locate_source(&servers, name)
            .unwrap_or_else(|| target_path(false, cwd, agent_dir))
    };

    let result = match edit {
        Edit::Remove => mcp_config::remove_server(&path, name).map(|removed| {
            if removed {
                println!("Removed MCP server \"{name}\" from {}", path.display());
                Ok(())
            } else {
                Err(())
            }
        }),
        Edit::SetEnabled(enabled) => mcp_config::set_enabled(&path, name, enabled).map(|changed| {
            if changed {
                let state = if enabled { "Enabled" } else { "Disabled" };
                println!("{state} MCP server \"{name}\" in {}", path.display());
                Ok(())
            } else {
                Err(())
            }
        }),
    };

    match result {
        Ok(Ok(())) => 0,
        Ok(Err(())) => {
            eprintln!(
                "{} MCP server \"{name}\" is not defined in {}",
                "Error:".red().bold(),
                path.display()
            );
            1
        }
        Err(e) => {
            eprintln!("{} {e}", "Error:".red().bold());
            1
        }
    }
}

fn locate_source(servers: &[LoadedServer], name: &str) -> Option<std::path::PathBuf> {
    servers
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.source.clone())
}
