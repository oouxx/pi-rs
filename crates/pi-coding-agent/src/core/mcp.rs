//! MCP (Model Context Protocol) client support.
//!
//! Lets ACP sessions call tools exposed by external MCP servers. The ACP
//! client sends a list of `McpServer` configs in `session/new` /
//! `session/load`; this module connects to them (stdio / streamable-HTTP),
//! enumerates their tools, and exposes them to pi as custom tools whose
//! `execute` forwards the call to the MCP server.
//!
//! This module is feature-gated behind `mcp` (default-on).

use std::sync::Arc;

use agent_client_protocol as acp;
use pi_agent_core::pi_ai_types::{image_block, text_block};

use crate::core::extensions::{ToolCallOutput, ToolDefinition};

/// A connected MCP server with its enumerated tools.
///
/// Dropping this disconnects from the server, so keep it alive for the
/// lifetime of the session that uses its tools.
pub struct McpConnection {
    /// Server name (as configured by the ACP client).
    pub name: String,
    peer: rmcp::Peer<rmcp::RoleClient>,
    /// Holds the running MCP client task; dropping it closes the connection.
    _running: rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
    /// Tools enumerated from this server.
    tools: Vec<rmcp::model::Tool>,
}

impl McpConnection {
    /// Connect to a single ACP `McpServer` config and enumerate its tools.
    pub async fn connect(config: &acp::McpServer) -> Result<Self, String> {
        let (name, peer, running) = match config {
            acp::McpServer::Stdio(s) => {
                let (peer, running) = connect_stdio(s).await?;
                (s.name.clone(), peer, running)
            }
            acp::McpServer::Http(h) => {
                let (peer, running) = connect_http(h).await?;
                (h.name.clone(), peer, running)
            }
            acp::McpServer::Sse(_) => {
                return Err("SSE MCP transport is not supported".to_string());
            }
            // Future ACP McpServer variants.
            _ => {
                return Err("Unsupported MCP server type".to_string());
            }
        };
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| format!("MCP server '{name}': list_tools failed: {e}"))?;
        Ok(Self {
            name,
            peer,
            _running: running,
            tools,
        })
    }

    /// Build pi `ToolDefinition`s for every tool this server exposes. Each
    /// tool's `execute` forwards the call to the MCP server via the peer.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let server_name = self.name.clone();
        let peer = self.peer.clone();
        self.tools
            .iter()
            .map(|tool| {
                let tool_name = tool.name.to_string();
                let tool_name_for_exec = tool_name.clone();
                let peer = peer.clone();
                let label = tool
                    .title
                    .clone()
                    .map(|t| format!("{server_name}: {t}"))
                    .unwrap_or_else(|| format!("{server_name}: {tool_name}"));
                let description = tool
                    .description
                    .clone()
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| format!("MCP tool '{tool_name}' from server '{server_name}'"));
                let parameters = Some(serde_json::Value::Object(tool.input_schema.as_ref().clone()));
                let execute: pi_extension_api::ToolExecuteFn = Arc::new(
                    move |_tool_call_id: String,
                          params: serde_json::Value,
                          _signal: Option<tokio::sync::watch::Receiver<bool>>| {
                        let peer = peer.clone();
                        let tool_name = tool_name_for_exec.clone();
                        Box::pin(async move {
                            let args = match params {
                                serde_json::Value::Object(map) => map,
                                _ => serde_json::Map::new(),
                            };
                            let request =
                                rmcp::model::CallToolRequestParams::new(tool_name.clone())
                                    .with_arguments(args);
                            let result = peer
                                .call_tool(request)
                                .await
                                .map_err(|e| format!("MCP call '{tool_name}' failed: {e}"))?;
                            Ok(call_result_to_output(result))
                        })
                    },
                );
                ToolDefinition {
                    name: tool_name,
                    label: Some(label),
                    description,
                    prompt_snippet: None,
                    prompt_guidelines: None,
                    parameters,
                    render_shell: None,
                    execution_mode: Some("sequential".to_string()),
                    execute: Some(execute),
                }
            })
            .collect()
    }
}

/// Connect to a stdio MCP server (spawn its command as a child process).
async fn connect_stdio(
    server: &acp::McpServerStdio,
) -> Result<
    (
        rmcp::Peer<rmcp::RoleClient>,
        rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
    ),
    String,
> {
    use rmcp::transport::{which_command, TokioChildProcess};
    let mut cmd = which_command(&server.command)
        .map_err(|e| format!("MCP server '{}': command not found: {e}", server.name))?;
    cmd.args(&server.args);
    for var in &server.env {
        cmd.env(&var.name, &var.value);
    }
    let process = TokioChildProcess::new(cmd)
        .map_err(|e| format!("MCP server '{}': spawn failed: {e}", server.name))?;
    let running = rmcp::serve_client(rmcp::model::ClientInfo::default(), process)
        .await
        .map_err(|e| format!("MCP server '{}': handshake failed: {e}", server.name))?;
    Ok((running.peer().clone(), running))
}

/// Connect to a streamable-HTTP MCP server.
async fn connect_http(
    server: &acp::McpServerHttp,
) -> Result<
    (
        rmcp::Peer<rmcp::RoleClient>,
        rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
    ),
    String,
> {
    use rmcp::transport::StreamableHttpClientTransport;
    let transport = StreamableHttpClientTransport::from_uri(server.url.clone());
    let running = rmcp::serve_client(rmcp::model::ClientInfo::default(), transport)
        .await
        .map_err(|e| format!("MCP server '{}': handshake failed: {e}", server.name))?;
    Ok((running.peer().clone(), running))
}

/// Convert an MCP `CallToolResult` into a pi `ToolCallOutput`.
fn call_result_to_output(result: rmcp::model::CallToolResult) -> ToolCallOutput {
    // Prefer the structured result when present, otherwise flatten content.
    let mut content: Vec<serde_json::Value> = Vec::new();
    if let Some(structured) = result.structured_content {
        content.push(structured);
    } else {
        for block in &result.content {
            content.push(content_block_to_value(block));
        }
    }
    ToolCallOutput {
        content,
        details: Some(serde_json::json!({
            "isError": result.is_error.unwrap_or(false),
        })),
        is_error: result.is_error.unwrap_or(false),
        terminate: None,
    }
}

/// Serialize an MCP content block into a pi `ContentBlock`-shaped JSON value.
fn content_block_to_value(content: &rmcp::model::Content) -> serde_json::Value {
    let raw: &rmcp::model::RawContent = content;
    if let Some(t) = raw.as_text() {
        return serde_json::to_value(text_block(t.text.clone())).unwrap_or_default();
    }
    if let Some(img) = raw.as_image() {
        return serde_json::to_value(image_block(img.data.clone(), img.mime_type.clone()))
            .unwrap_or_default();
    }
    if let Some(res) = raw.as_resource() {
        let text = match &res.resource {
            rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
            _ => String::new(),
        };
        return serde_json::to_value(text_block(text)).unwrap_or_default();
    }
    // Fallback: serialize the whole block to a string.
    serde_json::to_value(text_block(serde_json::to_string(content).unwrap_or_default()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Build a `CallToolResult` from JSON (the struct is non-exhaustive, so
    /// construct it the same way the wire does).
    fn result_from_json(v: serde_json::Value) -> rmcp::model::CallToolResult {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn text_content_becomes_text_block() {
        let content = rmcp::model::Content::new(rmcp::model::RawContent::text("hello"), None);
        let v = content_block_to_value(&content);
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hello");
    }

    #[test]
    fn call_result_content_flatmaps_text() {
        let result = result_from_json(serde_json::json!({
            "content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}],
            "isError": false
        }));
        let output = call_result_to_output(result);
        assert!(!output.is_error);
        assert_eq!(output.content.len(), 2);
        assert_eq!(output.content[0]["text"], "a");
        assert_eq!(output.content[1]["text"], "b");
    }

    #[test]
    fn call_result_prefers_structured_content() {
        let result = result_from_json(serde_json::json!({
            "content": [],
            "structuredContent": {"answer": 42},
            "isError": false
        }));
        let output = call_result_to_output(result);
        assert_eq!(output.content, vec![serde_json::json!({"answer": 42})]);
    }

    #[test]
    fn call_result_error_flag_forwarded() {
        let result = result_from_json(serde_json::json!({
            "content": [{"type": "text", "text": "boom"}],
            "isError": true
        }));
        let output = call_result_to_output(result);
        assert!(output.is_error);
        assert_eq!(output.details.as_ref().unwrap()["isError"], true);
    }

    /// Full round-trip against the Python stdio test server at
    /// `/tmp/mcp_test_server.py` (not portable, so `#[ignore]`d by default).
    /// Run with `cargo test -p pi-coding-agent --lib -- --ignored`.
    #[tokio::test]
    #[ignore = "requires /tmp/mcp_test_server.py (see ACP docs)"]
    async fn stdio_server_tool_round_trip() {
        let server = acp::McpServer::Stdio(
            acp::McpServerStdio::new("test-mcp", "/tmp/mcp_test_server.py"),
        );
        let conn = McpConnection::connect(&server).await.expect("connect");
        let defs = conn.tool_definitions();
        assert_eq!(defs.len(), 2, "server exposes echo + add");

        // echo tool
        let echo = defs.iter().find(|d| d.name == "echo").expect("echo tool");
        let exec = echo.execute.clone().expect("echo execute");
        let fut = (exec)("id1".to_string(), serde_json::json!({ "text": "hi" }), None);
        let out = fut.await.expect("call echo");
        assert_eq!(out.content[0]["text"], "echo:hi");

        // add tool
        let add = defs.iter().find(|d| d.name == "add").expect("add tool");
        let exec = add.execute.clone().expect("add execute");
        let fut = (exec)("id2".to_string(), serde_json::json!({ "a": 2, "b": 3 }), None);
        let out = fut.await.expect("call add");
        assert_eq!(out.content[0]["text"], "sum:5");
    }
}
