//! MCP (Model Context Protocol) client support.
//!
//! pi connects to MCP servers from two sources:
//!
//! - `mcp.json` in the agent directory and the project `{cwd}/.pi-rs/mcp.json`
//!   (see [`crate::core::mcp_config`]); and
//! - the `mcpServers` an ACP client sends in `session/new` / `session/load`.
//!
//! Both are normalized to [`McpServerSpec`]. Connecting enumerates each
//! server's tools and exposes them to pi as custom tools whose `execute`
//! forwards the call to the server over stdio, streamable HTTP, or the legacy
//! SSE transport.
//!
//! This module is feature-gated behind `mcp` (default-on).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol as acp;
use pi_agent_core::pi_ai_types::{image_block, text_block};

use crate::core::extensions::{ToolCallOutput, ToolDefinition};

/// How long a single server connection may take during startup.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How an MCP server is reached.
#[derive(Debug, Clone)]
pub enum McpTransportSpec {
    /// Spawn a child process and speak JSON-RPC over its stdio.
    Stdio {
        /// Executable path or name resolved on `PATH`.
        command: String,
        /// Command-line arguments.
        args: Vec<String>,
        /// Extra environment variables for the child process.
        env: BTreeMap<String, String>,
        /// Working directory for the child process (defaults to the session cwd).
        cwd: Option<String>,
    },
    /// Streamable HTTP transport (protocol revision 2025-03-26).
    Http {
        /// Server URL.
        url: String,
        /// Extra HTTP headers sent with every request.
        headers: BTreeMap<String, String>,
    },
    /// Legacy HTTP+SSE transport (protocol revision 2024-11-05).
    Sse {
        /// SSE endpoint URL.
        url: String,
        /// Extra HTTP headers sent with every request.
        headers: BTreeMap<String, String>,
    },
}

/// A named MCP server to connect to.
#[derive(Debug, Clone)]
pub struct McpServerSpec {
    /// Name used for log/error messages (not part of the tool name).
    pub name: String,
    /// How the server is reached.
    pub transport: McpTransportSpec,
}

impl McpServerSpec {
    /// Map an ACP `McpServer` config onto a spec.
    pub fn from_acp(config: &acp::McpServer) -> Self {
        let headers = |headers: &[acp::HttpHeader]| -> BTreeMap<String, String> {
            headers
                .iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect()
        };
        match config {
            acp::McpServer::Stdio(s) => Self {
                name: s.name.clone(),
                transport: McpTransportSpec::Stdio {
                    command: s.command.to_string_lossy().to_string(),
                    args: s.args.clone(),
                    env: s
                        .env
                        .iter()
                        .map(|v| (v.name.clone(), v.value.clone()))
                        .collect(),
                    cwd: None,
                },
            },
            acp::McpServer::Http(h) => Self {
                name: h.name.clone(),
                transport: McpTransportSpec::Http {
                    url: h.url.clone(),
                    headers: headers(&h.headers),
                },
            },
            acp::McpServer::Sse(s) => Self {
                name: s.name.clone(),
                transport: McpTransportSpec::Sse {
                    url: s.url.clone(),
                    headers: headers(&s.headers),
                },
            },
            // Future ACP McpServer variants: unreachable today.
            _ => Self {
                name: "unknown".to_string(),
                transport: McpTransportSpec::Stdio {
                    command: String::new(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            },
        }
    }
}

/// A connected MCP server with its enumerated tools.
///
/// The tool-execute closures built by [`McpConnection::tool_definitions`] hold
/// an `Arc` to the same inner state, so a connection stays alive as long as one
/// of its tools is reachable even if the `McpConnection` handle is dropped.
pub struct McpConnection {
    inner: Arc<McpInner>,
}

struct McpInner {
    /// Server name.
    name: String,
    /// Handle used to call tools and send notifications.
    peer: rmcp::Peer<rmcp::RoleClient>,
    /// Owns the running client task; dropped with the inner state closes the
    /// connection.
    _running: rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
    /// Tools enumerated from this server at connect time.
    tools: Vec<rmcp::model::Tool>,
}

impl Clone for McpConnection {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl McpConnection {
    /// Name of the connected server.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Number of tools the server offers.
    pub fn tool_count(&self) -> usize {
        self.inner.tools.len()
    }

    /// Connect to a single ACP `McpServer` config and enumerate its tools.
    pub async fn connect(config: &acp::McpServer) -> Result<Self, String> {
        Self::connect_spec(&McpServerSpec::from_acp(config)).await
    }

    /// Connect to a server spec and enumerate its tools.
    pub async fn connect_spec(spec: &McpServerSpec) -> Result<Self, String> {
        let (peer, running) = match &spec.transport {
            McpTransportSpec::Stdio {
                command,
                args,
                env,
                cwd,
            } => connect_stdio(&spec.name, command, args, env, cwd.as_deref()).await?,
            McpTransportSpec::Http { url, headers } => connect_http(&spec.name, url, headers).await?,
            McpTransportSpec::Sse { url, headers } => connect_sse(&spec.name, url, headers).await?,
        };
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| format!("MCP server '{}': list_tools failed: {e}", spec.name))?;
        Ok(Self {
            inner: Arc::new(McpInner {
                name: spec.name.clone(),
                peer,
                _running: running,
                tools,
            }),
        })
    }

    /// Build pi `ToolDefinition`s for every tool this server exposes. Each
    /// tool's `execute` forwards the call to the MCP server via the peer.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let server_name = self.inner.name.clone();
        self.inner
            .tools
            .iter()
            .map(|tool| {
                let tool_name = tool.name.to_string();
                let tool_name_for_exec = tool_name.clone();
                let inner = Arc::clone(&self.inner);
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
                        let inner = Arc::clone(&inner);
                        let tool_name = tool_name_for_exec.clone();
                        Box::pin(async move {
                            let args = match params {
                                serde_json::Value::Object(map) => map,
                                _ => serde_json::Map::new(),
                            };
                            let request =
                                rmcp::model::CallToolRequestParams::new(tool_name.clone())
                                    .with_arguments(args);
                            let result = inner
                                .peer
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
                    constrained_sampling: None,
                    render_shell: None,
                    execution_mode: Some("sequential".to_string()),
                    execute: Some(execute),
                    source_info: None,
                }
            })
            .collect()
    }
}

/// Connect every spec, returning the built tools, the connections (for status
/// and keep-alive), and one error message per server that failed.
///
/// Failures are reported, never fatal: a session starts even when a server is
/// down.
pub async fn connect_specs(
    specs: &[McpServerSpec],
) -> (Vec<ToolDefinition>, Vec<McpConnection>, Vec<String>) {
    let mut tools = Vec::new();
    let mut connections = Vec::new();
    let mut errors = Vec::new();
    for spec in specs {
        match tokio::time::timeout(CONNECT_TIMEOUT, McpConnection::connect_spec(spec)).await {
            Ok(Ok(connection)) => {
                tools.extend(connection.tool_definitions());
                connections.push(connection);
            }
            Ok(Err(e)) => errors.push(e),
            Err(_) => errors.push(format!(
                "MCP server '{}': timed out after {}s",
                spec.name,
                CONNECT_TIMEOUT.as_secs()
            )),
        }
    }
    (tools, connections, errors)
}

/// Connect to a stdio MCP server (spawn its command as a child process).
async fn connect_stdio(
    name: &str,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> Result<
    (
        rmcp::Peer<rmcp::RoleClient>,
        rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
    ),
    String,
> {
    use rmcp::transport::{which_command, TokioChildProcess};
    let mut cmd = which_command(command)
        .map_err(|e| format!("MCP server '{name}': command not found: {e}"))?;
    cmd.args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let process = TokioChildProcess::new(cmd)
        .map_err(|e| format!("MCP server '{name}': spawn failed: {e}"))?;
    let running = rmcp::serve_client(rmcp::model::ClientInfo::default(), process)
        .await
        .map_err(|e| format!("MCP server '{name}': handshake failed: {e}"))?;
    Ok((running.peer().clone(), running))
}

/// Connect to a streamable-HTTP MCP server.
async fn connect_http(
    name: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
) -> Result<
    (
        rmcp::Peer<rmcp::RoleClient>,
        rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
    ),
    String,
> {
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    use rmcp::transport::StreamableHttpClientTransport;
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_string());
    let config = if headers.is_empty() {
        config
    } else {
        config.custom_headers(to_header_map(headers)?)
    };
    let transport = StreamableHttpClientTransport::from_config(config);
    let running = rmcp::serve_client(rmcp::model::ClientInfo::default(), transport)
        .await
        .map_err(|e| format!("MCP server '{name}': handshake failed: {e}"))?;
    Ok((running.peer().clone(), running))
}

/// Connect to a legacy SSE MCP server.
async fn connect_sse(
    name: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
) -> Result<
    (
        rmcp::Peer<rmcp::RoleClient>,
        rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
    ),
    String,
> {
    let transport = crate::core::mcp_sse::SseClientTransport::connect(url, headers.clone())
        .await
        .map_err(|e| format!("MCP server '{name}': {e}"))?;
    let running = rmcp::serve_client(rmcp::model::ClientInfo::default(), transport)
        .await
        .map_err(|e| format!("MCP server '{name}': handshake failed: {e}"))?;
    Ok((running.peer().clone(), running))
}

/// Build an HTTP header map from string pairs, rejecting invalid names/values.
fn to_header_map(
    headers: &BTreeMap<String, String>,
) -> Result<std::collections::HashMap<reqwest::header::HeaderName, reqwest::header::HeaderValue>, String> {
    let mut map = std::collections::HashMap::new();
    for (name, value) in headers {
        let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| format!("invalid header name '{name}': {e}"))?;
        let header_value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|e| format!("invalid header value for '{name}': {e}"))?;
        map.insert(header_name, header_value);
    }
    Ok(map)
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

    #[test]
    fn acp_sse_server_maps_to_sse_spec() {
        let server = acp::McpServer::Sse(acp::McpServerSse::new(
            "docs",
            "https://example.com/sse",
        ));
        let spec = McpServerSpec::from_acp(&server);
        assert_eq!(spec.name, "docs");
        match spec.transport {
            McpTransportSpec::Sse { url, .. } => assert_eq!(url, "https://example.com/sse"),
            _ => panic!("expected Sse transport"),
        }
    }

    #[test]
    fn invalid_header_value_is_rejected() {
        let mut headers = BTreeMap::new();
        headers.insert("x-bad".to_string(), "line\nbreak".to_string());
        assert!(to_header_map(&headers).is_err());
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
