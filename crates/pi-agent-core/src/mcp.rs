#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unnecessary_struct_initialization,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_closure,
    clippy::missing_const_for_fn,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::manual_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::redundant_clone,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::match_same_arms,
    clippy::bool_to_int_with_if,
    clippy::needless_continue,
    clippy::items_after_statements,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::derive_partial_eq_without_eq,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_lit_as_bytes,
    clippy::trivially_copy_pass_by_ref,
    clippy::single_char_pattern,
    clippy::format_push_string,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::needless_raw_string_hashes,
    clippy::unnecessary_fold,
    clippy::needless_pass_by_ref_mut,
    clippy::map_identity,
    clippy::needless_return_with_question_mark,
    clippy::needless_lifetimes,
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::enum_glob_use,
    clippy::future_not_send,
    clippy::should_implement_trait,
    clippy::new_without_default,
    clippy::return_self_not_must_use,
    clippy::use_self,












    clippy::significant_drop_tightening,

    clippy::default_trait_access,

    clippy::iter_with_drain,

    clippy::if_not_else,

    clippy::explicit_iter_loop,

    clippy::assigning_clones,

    clippy::implicit_hasher,

    clippy::ignored_unit_patterns,

    clippy::missing_fields_in_debug,

    clippy::or_fun_call,

    clippy::too_long_first_doc_paragraph,

    clippy::manual_string_new,

    clippy::single_match_else,

    clippy::significant_drop_in_scrutinee,

    clippy::needless_collect,

    clippy::duplicated_attributes,

)]
//! MCP (Model Context Protocol) integration for pi-agent-core.
//!
//! Reads `.mcp.json` config, connects to MCP servers, and exposes their tools
//! as `DynTool` instances. Available behind the `mcp` feature flag.

#![cfg(feature = "mcp")]

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{CallToolRequestParams, RawContent};
use rmcp::service::{Peer, RoleClient, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::ServiceExt;
use serde::{Deserialize, Serialize};

use crate::pi_ai_types::ContentBlock;
use crate::types::{AgentTool, AgentToolResult, DynTool};

// ============================================================
// Errors
// ============================================================

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("config error: {0}")]
    Config(String),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("tool call error: {0}")]
    ToolCall(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// ============================================================
// Config types
// ============================================================

/// Parsed `.mcp.json` configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

/// A single MCP server entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerConfig {
    /// Stdio-based server (e.g. `npx -y @modelcontextprotocol/server-filesystem`)
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    /// HTTP/SSE-based server
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

// ============================================================
// Result types
// ============================================================

/// Loaded MCP tools and their connection handle.
pub struct McpTools {
    /// MCP tools wrapped as `DynTool` instances.
    pub tools: Vec<DynTool>,
    /// Handle that keeps MCP server connections alive.
    /// Drop this to disconnect all servers.
    pub handle: McpHandle,
}

/// Manages MCP server lifecycle.
pub struct McpHandle {
    connections: Arc<Vec<McpServerConnection>>,
}

impl Drop for McpHandle {
    fn drop(&mut self) {
        // Connections are dropped automatically when `connections` refcount reaches zero.
        // RunningService drop cancels the service and closes the transport.
    }
}

impl McpHandle {
    /// Number of connected MCP servers.
    pub fn server_count(&self) -> usize {
        self.connections.len()
    }

    /// Names of connected MCP servers.
    pub fn server_names(&self) -> Vec<String> {
        self.connections.iter().map(|c| c.name.clone()).collect()
    }
}

#[allow(dead_code)]
struct McpServerConnection {
    name: String,
    peer: Peer<RoleClient>,
    _service: RunningService<RoleClient, ()>,
    tool_names: Vec<String>,
}

// ============================================================
// Public API
// ============================================================

/// Load MCP tools from a `.mcp.json` config file.
///
/// Connects to all configured MCP servers, discovers their tools, and wraps each
/// as a `DynTool`. The returned `McpHandle` keeps connections alive.
pub async fn load_mcp_tools(path: &str) -> Result<McpTools, McpError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(McpError::Io)?;
    let config: McpConfig =
        serde_json::from_str(&content).map_err(|e| McpError::Config(e.to_string()))?;
    load_mcp_tools_from_config(&config).await
}

/// Load MCP tools from a parsed `McpConfig`.
pub async fn load_mcp_tools_from_config(config: &McpConfig) -> Result<McpTools, McpError> {
    let mut all_tools: Vec<DynTool> = Vec::new();
    let mut connections = Vec::new();

    for (name, server_config) in &config.mcp_servers {
        match server_config {
            McpServerConfig::Stdio { command, args, env } => {
                let transport = build_stdio_transport(command, args, env)?;

                let service = ()
                    .serve(transport)
                    .await
                    .map_err(|e| McpError::Connection(format!("{}: {}", name, e)))?;

                let peer = service.peer().clone();

                let tools = peer
                    .list_all_tools()
                    .await
                    .map_err(|e| McpError::Connection(format!("{}: {}", name, e)))?;

                let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

                for tool in &tools {
                    all_tools.push(wrap_mcp_tool(
                        name.clone(),
                        tool,
                        peer.clone(),
                    ));
                }

                connections.push(McpServerConnection {
                    name: name.clone(),
                    peer,
                    _service: service,
                    tool_names,
                });
            }
            McpServerConfig::Http { url, .. } => {
                let transport = StreamableHttpClientTransport::from_config(
                    StreamableHttpClientTransportConfig::with_uri(url.clone()),
                );

                let service = ()
                    .serve(transport)
                    .await
                    .map_err(|e| McpError::Connection(format!("{}: {}", name, e)))?;

                let peer = service.peer().clone();

                let tools = peer
                    .list_all_tools()
                    .await
                    .map_err(|e| McpError::Connection(format!("{}: {}", name, e)))?;

                let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

                for tool in &tools {
                    all_tools.push(wrap_mcp_tool(name.clone(), tool, peer.clone()));
                }

                connections.push(McpServerConnection {
                    name: name.clone(),
                    peer,
                    _service: service,
                    tool_names,
                });
            }
        }
    }

    Ok(McpTools {
        tools: all_tools,
        handle: McpHandle {
            connections: Arc::new(connections),
        },
    })
}

// ============================================================
// Internal: stdio transport builder
// ============================================================

fn build_stdio_transport(
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> Result<TokioChildProcess, McpError> {
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args);
    for (key, val) in env {
        cmd.env(key, val);
    }
    let transport = TokioChildProcess::new(cmd)?;
    Ok(transport)
}

// ============================================================
// Internal: wrap an MCP tool as DynTool
// ============================================================

fn wrap_mcp_tool(
    server_name: String,
    tool: &rmcp::model::Tool,
    peer: Peer<RoleClient>,
) -> DynTool {
    let tool_name = tool.name.to_string();
    let label = if tool.description.is_some() {
        format!("{}@{}", tool_name, server_name)
    } else {
        tool_name.clone()
    };

    AgentTool {
        name: format!("{}__{}", server_name, tool_name),
        description: tool
            .description
            .as_ref()
            .map(|d| d.to_string())
            .unwrap_or_else(|| format!("MCP tool from {server_name}")),
        label,
        parameters_schema: serde_json::Value::Object((*tool.input_schema).clone()),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_id, params: serde_json::Value, _signal, _on_update| {
            let peer = peer.clone();
            let name = tool_name.clone();
            Box::pin(async move {
                let arguments = params.as_object().cloned();

                let result = peer
                    .call_tool(CallToolRequestParams::new(name.clone()).with_arguments(arguments.unwrap_or_default()))
                    .await
                    .map_err(|e| {
                        let err: Box<dyn std::error::Error + Send + Sync> =
                            format!("MCP tool call failed: {e}").into();
                        err
                    })?;

                let content = convert_mcp_content(&result);

                Ok(AgentToolResult {
                    content,
                    details: serde_json::Value::Null,
                    terminate: None,
                })
            })
        }),
    }
}

// ============================================================
// Internal: convert MCP tool result content to ContentBlock
// ============================================================

fn convert_mcp_content(result: &rmcp::model::CallToolResult) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();

    for item in &result.content {
        match &item.raw {
            RawContent::Text(text_content) => {
                blocks.push(ContentBlock::text(text_content.text.clone()));
            }
            RawContent::Image(image_content) => {
                blocks.push(ContentBlock::Image {
                    data: image_content.data.clone(),
                    mime_type: image_content.mime_type.clone(),
                });
            }
            _other => {
                blocks.push(ContentBlock::text("(MCP content: non-text/non-image result)"));
            }
        }
    }

    if blocks.is_empty() {
        if result.is_error.unwrap_or(false) {
            blocks.push(ContentBlock::text("MCP tool returned an error (no details)"));
        } else {
            blocks.push(ContentBlock::text("Tool completed successfully (no output)"));
        }
    }

    blocks
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_config_deserialize_stdio() {
        let json = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                    "env": { "KEY": "val" }
                }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mcp_servers.len(), 1);
        let server = config.mcp_servers.get("filesystem").unwrap();
        match server {
            McpServerConfig::Stdio { command, args, env } => {
                assert_eq!(command, "npx");
                assert!(args.contains(&"-y".to_string()));
                assert_eq!(env.get("KEY"), Some(&"val".to_string()));
            }
            _ => panic!("expected Stdio variant"),
        }
    }

    #[test]
    fn test_mcp_config_deserialize_http() {
        let json = r#"{
            "mcpServers": {
                "remote": {
                    "url": "https://example.com/mcp",
                    "headers": { "Authorization": "Bearer token" }
                }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(json).unwrap();
        let server = config.mcp_servers.get("remote").unwrap();
        match server {
            McpServerConfig::Http { url, headers } => {
                assert_eq!(url, "https://example.com/mcp");
                assert_eq!(headers.get("Authorization"), Some(&"Bearer token".to_string()));
            }
            _ => panic!("expected Http variant"),
        }
    }

    #[test]
    fn test_mcp_config_empty_servers() {
        let json = r#"{}"#;
        let config: McpConfig = serde_json::from_str(json).unwrap();
        assert!(config.mcp_servers.is_empty());
    }

    #[test]
    fn test_mcp_config_no_mcp_servers_key() {
        let json = r#"{"someOtherConfig": true}"#;
        let config: McpConfig = serde_json::from_str(json).unwrap();
        assert!(config.mcp_servers.is_empty());
    }

    #[test]
    fn test_mcp_config_stdio_defaults() {
        let json = r#"{
            "mcpServers": {
                "simple": {
                    "command": "echo"
                }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(json).unwrap();
        let server = config.mcp_servers.get("simple").unwrap();
        match server {
            McpServerConfig::Stdio { command, args, env } => {
                assert_eq!(command, "echo");
                assert!(args.is_empty());
                assert!(env.is_empty());
            }
            _ => panic!("expected Stdio variant"),
        }
    }
}
