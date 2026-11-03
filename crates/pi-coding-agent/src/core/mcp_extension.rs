//! Built-in `/mcp` command.
//!
//! pi-rs exposes the manager as a menu sequence over `ExtensionUIContext`
//! (`select` / `input` / `notify`) instead of TS's custom `McpManagerView`;
//! the state machine is the same: pick a server, then sign in/out, reconnect,
//! enable/disable, or change exposure.

use std::sync::Arc;

use pi_extension_api::hook::{CommandRegistration, CommandRegistry, HookHandler};
use pi_extension_api::ExtensionContext;
use serde_json::json;

use crate::core::mcp_config::{self, LoadedServer, McpServerConfigPatch, McpScope};
use crate::core::mcp_oauth::{
    self, McpOAuthCredentialStore, McpOAuthSettings, McpSignInPrompt,
};
use crate::core::mcp_runtime::McpServerConnection;
use crate::core::mcp_servers::McpExposure;

const USAGE: &str = "Usage: /mcp, /mcp login [server], /mcp logout [server], /mcp reconnect [server]";

/// The built-in MCP command extension.
#[derive(Debug, Default)]
pub struct McpExtension;

impl McpExtension {
    /// Create the extension.
    pub fn new() -> Self {
        Self
    }
}

fn notify(ctx: &ExtensionContext, message: &str) {
    (ctx.ui.notify)(message, &json!({}));
}

fn runtime_strings(ctx: &ExtensionContext) -> (String, String) {
    let cwd = (ctx.runtime.get_cwd)();
    let agent_dir = (ctx.runtime.get_agent_dir)();
    (cwd, agent_dir)
}

fn find_server<'a>(servers: &'a [LoadedServer], name: &str) -> Option<&'a LoadedServer> {
    servers.iter().find(|server| server.name == name)
}

fn oauth_settings(server: &LoadedServer) -> Result<McpOAuthSettings, String> {
    McpOAuthSettings::from_config(server.config.oauth.as_ref(), &server.name)
}

fn server_url(server: &LoadedServer) -> Result<url::Url, String> {
    server
        .config
        .url
        .as_deref()
        .and_then(|url| url::Url::parse(url).ok())
        .ok_or_else(|| format!("MCP server \"{}\" has no valid URL", server.name))
}

async fn sign_in(
    ctx: &ExtensionContext,
    server: &LoadedServer,
    cwd: &str,
    agent_dir: &str,
) -> Result<(), String> {
    if !mcp_oauth::server_uses_oauth(&server.config) {
        return Err(format!(
            "MCP server \"{}\" does not use OAuth; only HTTP servers without an Authorization header do.",
            server.name
        ));
    }
    let url = server_url(server)?;
    let settings = oauth_settings(server)?;
    let store = McpOAuthCredentialStore::new(std::path::Path::new(agent_dir))
        .for_server(&server.name, url.as_str());
    let ui = ctx.ui.clone();
    let name = server.name.clone();
    let prompt = McpSignInPrompt {
        show_authorization_url: Arc::new(move |authorization_url| {
            (ui.notify)(
                &format!("Sign in to MCP server \"{name}\" in your browser:\n{authorization_url}"),
                &json!({}),
            );
            crate::utils::open_browser::open_browser(authorization_url.as_str());
        }),
        prompt_for_redirect_url: {
            let ui = ctx.ui.clone();
            Arc::new(move |_cancel| {
                let ui = ui.clone();
                Box::pin(async move {
                    (ui.input)(
                        "Waiting for sign-in. If the browser cannot reach this machine, paste the URL it was redirected to.",
                        Some("http://127.0.0.1:.../callback?code=..."),
                        None,
                    )
                    .await
                })
            })
        },
    };
    mcp_oauth::sign_in_mcp_server(&url, store, &settings, None, prompt, None)
        .await
        .map_err(|error| error.to_string())?;
    // Reconnect with the fresh credentials so the current session sees them.
    let connection = McpServerConnection::with_agent_dir(server.clone(), cwd, agent_dir);
    let _ = connection.get_client().await;
    connection.close().await;
    Ok(())
}

async fn reconnect(
    ctx: &ExtensionContext,
    server: &LoadedServer,
    cwd: &str,
    agent_dir: &str,
) -> String {
    let connection = McpServerConnection::with_agent_dir(server.clone(), cwd, agent_dir);
    let result = match connection.get_client().await {
        Ok(_) => format!(
            "Connected to MCP server \"{}\" ({} tools).",
            server.name,
            connection.tools().len()
        ),
        Err(error) => error,
    };
    connection.close().await;
    let _ = ctx;
    result
}

async fn choose_exposure(ctx: &ExtensionContext, _server: &LoadedServer) -> Option<McpExposure> {
    let options = vec![
        "codemode".to_string(),
        "deferred".to_string(),
        "direct".to_string(),
        "hidden".to_string(),
    ];
    let choice = (ctx.ui.select)("Exposure", &options, None).await?;
    McpExposure::parse(&choice)
}

async fn manage(ctx: &ExtensionContext) {
    let (cwd, agent_dir) = runtime_strings(ctx);
    loop {
        let loaded = mcp_config::load_mcp_config(&cwd, &agent_dir);
        if !loaded.errors.is_empty() {
            notify(ctx, &format!("MCP config errors:\n{}", loaded.errors.join("\n")));
        }
        if loaded.servers.is_empty() {
            notify(ctx, "No MCP servers configured. Add them to mcp.json or .pi-rs/mcp.json.");
            return;
        }
        let names: Vec<String> = loaded.servers.iter().map(|server| server.name.clone()).collect();
        let Some(name) = (ctx.ui.select)("MCP server", &names, None).await else {
            return;
        };
        let Some(server) = find_server(&loaded.servers, &name).cloned() else {
            continue;
        };
        let actions = vec![
            "Sign in".to_string(),
            "Sign out".to_string(),
            "Reconnect".to_string(),
            "Enable".to_string(),
            "Disable".to_string(),
            "Exposure".to_string(),
        ];
        let Some(action) = (ctx.ui.select)(
            &format!(
                "MCP server {} ({}, {})",
                server.name,
                server.config.exposure(),
                if server.config.is_enabled() {
                    "enabled"
                } else {
                    "disabled"
                }
            ),
            &actions,
            None,
        )
        .await
        else {
            continue;
        };
        match action.as_str() {
            "Sign in" => match sign_in(ctx, &server, &cwd, &agent_dir).await {
                Ok(()) => notify(ctx, &format!("Signed in to MCP server \"{}\".", server.name)),
                Err(error) => notify(ctx, &error),
            },
            "Sign out" => {
                if let Ok(url) = server_url(&server) {
                    let store = McpOAuthCredentialStore::new(std::path::Path::new(&agent_dir))
                        .for_server(&server.name, url.as_str());
                    let removed = store.remove();
                    let message = if removed {
                        format!("Signed out of MCP server \"{}\".", server.name)
                    } else {
                        format!("No stored credentials for MCP server \"{}\".", server.name)
                    };
                    notify(ctx, &message);
                }
            }
            "Reconnect" => {
                let message = reconnect(ctx, &server, &cwd, &agent_dir).await;
                notify(ctx, &message);
            }
            "Enable" | "Disable" => {
                let enabled = action == "Enable";
                let path = server
                    .override_path
                    .clone()
                    .unwrap_or_else(|| server.source.clone());
                let patch = McpServerConfigPatch {
                    enabled: Some(enabled),
                    exposure: None,
                };
                let result = mcp_config::update_server(
                    &path,
                    &server.name,
                    &patch,
                    server.override_path.is_some(),
                );
                match result {
                    Ok(_) => notify(
                        ctx,
                        &format!(
                            "{} MCP server \"{}\".",
                            if enabled { "Enabled" } else { "Disabled" },
                            server.name
                        ),
                    ),
                    Err(error) => notify(ctx, &error),
                }
            }
            "Exposure" => {
                let Some(exposure) = choose_exposure(ctx, &server).await else {
                    continue;
                };
                let path = server
                    .override_path
                    .clone()
                    .unwrap_or_else(|| server.source.clone());
                let patch = McpServerConfigPatch {
                    enabled: None,
                    exposure: Some(exposure),
                };
                match mcp_config::update_server(
                    &path,
                    &server.name,
                    &patch,
                    server.override_path.is_some(),
                ) {
                    Ok(_) => notify(
                        ctx,
                        &format!("Exposure of \"{}\" set to {exposure}.", server.name),
                    ),
                    Err(error) => notify(ctx, &error),
                }
            }
            _ => {}
        }
    }
}

async fn login_command(ctx: &ExtensionContext, name: Option<&str>) -> bool {
    let (cwd, agent_dir) = runtime_strings(ctx);
    let loaded = mcp_config::load_mcp_config(&cwd, &agent_dir);
    let server = match name {
        Some(name) => find_server(&loaded.servers, name).cloned(),
        None => {
            let names: Vec<String> = loaded
                .servers
                .iter()
                .filter(|server| mcp_oauth::server_uses_oauth(&server.config))
                .map(|server| server.name.clone())
                .collect();
            if names.is_empty() {
                notify(ctx, "No enabled MCP server uses OAuth.");
                return true;
            }
            (ctx.ui.select)("MCP server", &names, None)
                .await
                .and_then(|name| find_server(&loaded.servers, &name).cloned())
        }
    };
    let Some(server) = server else {
        notify(ctx, "No such MCP server.");
        return true;
    };
    match sign_in(ctx, &server, &cwd, &agent_dir).await {
        Ok(()) => notify(ctx, &format!("Signed in to MCP server \"{}\".", server.name)),
        Err(error) => notify(ctx, &error),
    }
    true
}

async fn logout_command(ctx: &ExtensionContext, name: Option<&str>) {
    let (cwd, agent_dir) = runtime_strings(ctx);
    let loaded = mcp_config::load_mcp_config(&cwd, &agent_dir);
    let server = match name {
        Some(name) => find_server(&loaded.servers, name).cloned(),
        None => {
            let names: Vec<String> = loaded
                .servers
                .iter()
                .filter(|server| mcp_oauth::server_uses_oauth(&server.config))
                .map(|server| server.name.clone())
                .collect();
            if names.is_empty() {
                notify(ctx, "No enabled MCP server uses OAuth.");
                return;
            }
            (ctx.ui.select)("MCP server", &names, None)
                .await
                .and_then(|name| find_server(&loaded.servers, &name).cloned())
        }
    };
    let Some(server) = server else {
        notify(ctx, "No such MCP server.");
        return;
    };
    let Ok(url) = server_url(&server) else {
        notify(ctx, &format!("MCP server \"{}\" has no valid URL.", server.name));
        return;
    };
    let store = McpOAuthCredentialStore::new(std::path::Path::new(&agent_dir))
        .for_server(&server.name, url.as_str());
    let removed = store.remove();
    let message = if removed {
        format!("Signed out of MCP server \"{}\".", server.name)
    } else {
        format!("No stored credentials for MCP server \"{}\".", server.name)
    };
    notify(ctx, &message);
}

async fn handle_command(args: &str, ctx: &ExtensionContext) {
    if !ctx.has_ui {
        notify(ctx, USAGE);
        return;
    }
    let mut parts = args.split_whitespace();
    let action = parts.next();
    let server = parts.next();
    match action {
        None => manage(ctx).await,
        Some("login") => {
            login_command(ctx, server).await;
        }
        Some("logout") => logout_command(ctx, server).await,
        Some("reconnect") => {
            let (cwd, agent_dir) = runtime_strings(ctx);
            let loaded = mcp_config::load_mcp_config(&cwd, &agent_dir);
            let target = match server {
                Some(name) => find_server(&loaded.servers, name).cloned(),
                None => {
                    let names: Vec<String> = loaded
                        .servers
                        .iter()
                        .map(|server| server.name.clone())
                        .collect();
                    if names.is_empty() {
                        notify(ctx, "No enabled MCP server to reconnect.");
                        return;
                    }
                    (ctx.ui.select)("MCP server", &names, None)
                        .await
                        .and_then(|name| find_server(&loaded.servers, &name).cloned())
                }
            };
            match target {
                Some(server) => {
                    let message = reconnect(ctx, &server, &cwd, &agent_dir).await;
                    notify(ctx, &message);
                }
                None => notify(ctx, "No such MCP server."),
            }
        }
        Some(_) => notify(ctx, USAGE),
    }
}

impl HookHandler for McpExtension {
    fn name(&self) -> &str {
        "mcp"
    }

    fn register_commands(&self, commands: &mut CommandRegistry) {
        commands.register(
            "mcp",
            CommandRegistration {
                description: "Manage MCP servers: sign in, reconnect, enable or disable, and change exposure"
                    .to_string(),
                execute: Arc::new(|args, ctx| {
                    let ctx = ctx.cloned();
                    Box::pin(async move {
                        if let Some(ctx) = ctx {
                            handle_command(&args, &ctx).await;
                        }
                    })
                }),
                get_argument_completions: None,
            },
        );
    }
}

/// Whether a server's manager entry is persisted to the project file.
#[allow(dead_code)]
fn is_project_override(server: &LoadedServer) -> bool {
    server.scope == McpScope::Global && server.override_path.is_some()
}
