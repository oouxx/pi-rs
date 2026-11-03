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

use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use colored::*;
use pi_coding_agent::config;
use pi_coding_agent::core::mcp_config::{
    self, global_config_path, project_config_path, LoadedServer, McpServerConfig,
};
use pi_coding_agent::core::mcp_oauth::{
    self, McpOAuthCredentialStore, McpOAuthSettings, McpSignInPrompt,
};
use pi_coding_agent::core::mcp_runtime::{McpServerConnection, McpServerState};
use pi_coding_agent::utils::open_browser::open_browser;
use tokio_util::sync::CancellationToken;
use url::Url;

const USAGE: &str = "\
Usage: pi-rs mcp <command>

Commands:
  list                                   List configured MCP servers
  add <name> -- <command> [args...]      Add a stdio server (project mcp.json)
  add <name> --url <url> [--header N:V]  Add an HTTP server (add --sse for legacy SSE)
  remove <name>                          Remove a server from the file that defines it
  enable <name>                          Enable a server
  disable <name>                         Disable a server
  login <name> [--timeout <seconds>]     Sign in to an OAuth MCP server
  logout <name>                          Delete stored OAuth credentials

Options:
  --global   Target the global {agent_dir}/mcp.json instead of the project file
  --exposure <mode>  Set codemode, deferred, direct, or hidden
  --description <text>  Describe what the server offers";

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
        "login" => cmd_login(&args[1..], &cwd, &agent_dir),
        "logout" => cmd_logout(&args[1..], &cwd, &agent_dir),
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

fn describe_transport(config: &McpServerConfig) -> String {
    if let Some(command) = &config.command {
        let mut parts = vec![command.clone()];
        parts.extend(config.args.clone().unwrap_or_default());
        return parts.join(" ");
    }
    let kind = if config.kind.as_deref() == Some("sse") {
        "sse"
    } else {
        "http"
    };
    format!("{kind} {}", config.url.as_deref().unwrap_or_default())
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
        let state = if server.config.is_enabled() {
            "enabled".green().to_string()
        } else {
            "disabled".dimmed().to_string()
        };
        println!(
            "{:<16} {:<8} {:<7} {:<8} {}",
            server.name,
            server.scope.as_str(),
            state,
            server.config.exposure(),
            describe_transport(&server.config)
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
    let mut exposure: Option<String> = None;
    let mut description: Option<String> = None;
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
        } else if !after_separator && arg == "--exposure" {
            i += 1;
            exposure = args.get(i).cloned();
        } else if !after_separator && arg == "--description" {
            i += 1;
            description = args.get(i).cloned();
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
                map.insert("headers".to_string(), serde_json::Value::Object(headers));
            }
            if let Some(exposure) = &exposure {
                map.insert("exposure".to_string(), serde_json::json!(exposure));
            }
            if let Some(description) = &description {
                map.insert("description".to_string(), serde_json::json!(description));
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
            if let Some(exposure) = &exposure {
                map.insert("exposure".to_string(), serde_json::json!(exposure));
            }
            if let Some(description) = &description {
                map.insert("description".to_string(), serde_json::json!(description));
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
        locate_source(&servers, name).unwrap_or_else(|| target_path(false, cwd, agent_dir))
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

fn cmd_login(args: &[String], cwd: &str, agent_dir: &str) -> i32 {
    let mut name = None;
    let mut timeout_secs = 300u64;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--timeout" => {
                index += 1;
                match args.get(index).and_then(|value| value.parse::<u64>().ok()) {
                    Some(seconds) if seconds > 0 => timeout_secs = seconds,
                    _ => {
                        eprintln!("{} --timeout must be a positive number of seconds", "Error:".red().bold());
                        return 1;
                    }
                }
            }
            value if name.is_none() => name = Some(value.to_string()),
            value => {
                eprintln!("{} unexpected argument {value}", "Error:".red().bold());
                return 1;
            }
        }
        index += 1;
    }
    let Some(name) = name else {
        eprintln!("{} Usage: pi-rs mcp login <server> [--timeout <seconds>]", "Error:".red().bold());
        return 1;
    };
    let loaded = mcp_config::load_mcp_config(cwd, agent_dir);
    let Some(server) = loaded.servers.iter().find(|server| server.name == name) else {
        eprintln!("{} No MCP server named \"{name}\"", "Error:".red().bold());
        return 1;
    };
    if !mcp_oauth::server_uses_oauth(&server.config) {
        eprintln!(
            "{} MCP server \"{name}\" does not use OAuth. Only HTTP servers without an Authorization header do.",
            "Error:".red().bold()
        );
        return 1;
    }
    let Some(url) = server.config.url.as_deref().and_then(|url| Url::parse(url).ok()) else {
        eprintln!("{} MCP server \"{name}\" has an invalid URL", "Error:".red().bold());
        return 1;
    };
    let settings = match McpOAuthSettings::from_config(server.config.oauth.as_ref(), &name) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("{} {error}", "Error:".red().bold());
            return 1;
        }
    };
    let store = McpOAuthCredentialStore::new(Path::new(agent_dir)).for_server(&name, url.as_str());
    let connection = McpServerConnection::with_agent_dir(server.clone(), cwd, agent_dir);
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{} {error}", "Error:".red().bold());
            return 1;
        }
    };
    runtime.block_on(async move {
        // Connecting first tells us whether a sign-in is actually needed.
        match connection.get_client().await {
            Ok(_) => {
                println!("Already signed in to MCP server \"{name}\".");
                connection.close().await;
                return 0;
            }
            Err(error) if connection.state() == McpServerState::NeedsAuth => {
                let _ = error;
            }
            Err(error) => {
                eprintln!("{} {error}", "Error:".red().bold());
                connection.close().await;
                return 1;
            }
        }
        let cancel = CancellationToken::new();
        let timeout_cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
            timeout_cancel.cancel();
        });
        let name_for_prompt = name.clone();
        let prompt = McpSignInPrompt {
            show_authorization_url: Arc::new(move |url| {
                println!(
                    "Sign in to MCP server \"{name_for_prompt}\" in your browser:\n{url}"
                );
                open_browser(url.as_str());
            }),
            prompt_for_redirect_url: Arc::new(|cancel| {
                Box::pin(async move {
                    if !std::io::stdin().is_terminal() {
                        cancel.cancelled().await;
                        return None;
                    }
                    tokio::task::spawn_blocking(move || {
                        println!(
                            "If the browser cannot reach this machine, paste the URL it was redirected to:"
                        );
                        let mut line = String::new();
                        std::io::stdin().read_line(&mut line).ok()?;
                        let line = line.trim();
                        if line.is_empty() {
                            None
                        } else {
                            Some(line.to_string())
                        }
                    })
                    .await
                    .ok()
                    .flatten()
                })
            }),
        };
        match mcp_oauth::sign_in_mcp_server(
            &url,
            store,
            &settings,
            None,
            prompt,
            Some(cancel),
        )
        .await
        {
            Ok(()) => {
                println!("Signed in to MCP server \"{name}\".");
                connection.close().await;
                0
            }
            Err(mcp_oauth::OAuthError::SignInCancelled) => {
                eprintln!(
                    "{} Sign-in to MCP server \"{name}\" was cancelled or timed out.",
                    "Error:".red().bold()
                );
                connection.close().await;
                1
            }
            Err(error) => {
                eprintln!("{} Sign-in failed: {error}", "Error:".red().bold());
                connection.close().await;
                1
            }
        }
    })
}

fn cmd_logout(args: &[String], cwd: &str, agent_dir: &str) -> i32 {
    let Some(name) = args.iter().find(|arg| !arg.starts_with("--")) else {
        eprintln!("{} Usage: pi-rs mcp logout <server>", "Error:".red().bold());
        return 1;
    };
    let loaded = mcp_config::load_mcp_config(cwd, agent_dir);
    let Some(server) = loaded.servers.iter().find(|server| server.name == *name) else {
        eprintln!("{} No MCP server named \"{name}\"", "Error:".red().bold());
        return 1;
    };
    let Some(url) = server.config.url.as_deref().and_then(|url| Url::parse(url).ok()) else {
        eprintln!("{} MCP server \"{name}\" has an invalid URL", "Error:".red().bold());
        return 1;
    };
    let store = McpOAuthCredentialStore::new(Path::new(agent_dir)).for_server(name, url.as_str());
    if store.remove() {
        println!("Signed out of MCP server \"{name}\".");
        0
    } else {
        println!("No stored credentials for MCP server \"{name}\".");
        0
    }
}

fn locate_source(servers: &[LoadedServer], name: &str) -> Option<std::path::PathBuf> {
    servers
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.source.clone())
}
