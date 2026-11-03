//! End-to-end test for the legacy MCP SSE transport.
//!
//! Spins up a minimal HTTP+SSE MCP server in-process (raw tokio TCP, no HTTP
//! framework) that implements just enough of the protocol (`initialize`,
//! `tools/list`, `tools/call`) and connects to it through
//! [`McpConnection::connect_spec`], proving the SSE client transport works:
//! GET stream + `endpoint` event + POST + JSON-RPC response over SSE.

#![cfg(feature = "mcp")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use pi_coding_agent::core::mcp_config::load_mcp_config;
use pi_coding_agent::core::mcp_runtime::McpServerConnection;
use pi_coding_agent::core::mcp_servers::{
    validate_mcp_server_config, LoadedServer, McpScope,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// A tiny SSE MCP server. Returns the URL of its `/sse` endpoint.
async fn spawn_mock_sse_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        // The SSE connection's sender; the POST handler pushes responses here.
        let (event_tx, event_rx) = mpsc::unbounded_channel::<String>();
        let event_tx = Arc::new(tokio::sync::Mutex::new(Some(event_tx)));
        let mut event_rx = Some(event_rx);
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let event_tx = Arc::clone(&event_tx);
            let event_rx = event_rx.take();
            tokio::spawn(async move {
                let _ = handle_connection(stream, event_tx, event_rx).await;
            });
        }
    });
    format!("http://{addr}/sse")
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    event_tx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<String>>>>,
    event_rx: Option<mpsc::UnboundedReceiver<String>>,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    let is_get = request_line.starts_with("GET ");
    if is_get {
        // Open the SSE stream with chunked encoding (needed for live streaming).
        write_half
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n",
            )
            .await?;
        write_chunk(&mut write_half, "event: endpoint\ndata: /messages\n\n").await?;

        let Some(mut rx) = event_rx else {
            return Ok(());
        };
        while let Some(message) = rx.recv().await {
            write_chunk(&mut write_half, &format!("data: {message}\n\n")).await?;
        }
        return Ok(());
    }

    // POST /messages: read the JSON-RPC request and answer over the SSE stream.
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await?;
    write_half
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
        .await?;

    let Ok(request) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return Ok(());
    };
    let Some(method) = request.get("method").and_then(|m| m.as_str()) else {
        return Ok(());
    };
    let Some(id) = request.get("id").cloned() else {
        return Ok(()); // notification
    };

    let response = match method {
        "initialize" => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "mock-sse", "version": "1.0.0" }
            }
        }),
        "tools/list" => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [{
                    "name": "echo",
                    "description": "Echo the text back",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "text": { "type": "string" } },
                        "required": ["text"]
                    }
                }]
            }
        }),
        "tools/call" => {
            let text = request
                .get("params")
                .and_then(|p| p.get("arguments"))
                .and_then(|a| a.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{ "type": "text", "text": format!("echo:{text}") }] }
            })
        }
        _ => serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "method not found" }
        }),
    };

    if let Some(tx) = event_tx.lock().await.as_ref() {
        let _ = tx.send(response.to_string());
    }
    Ok(())
}

async fn write_chunk(
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    data: &str,
) -> std::io::Result<()> {
    write_half
        .write_all(format!("{:x}\r\n{data}\r\n", data.len()).as_bytes())
        .await?;
    write_half.flush().await
}

#[tokio::test]
async fn sse_server_tool_round_trip() {
    let url = spawn_mock_sse_server().await;
    let config = validate_mcp_server_config(
        "mock",
        &serde_json::json!({"type": "sse", "url": url}),
    )
    .expect("valid config");
    let entry = LoadedServer {
        name: "mock".to_string(),
        config,
        source: std::path::PathBuf::from("<test>"),
        scope: McpScope::Extension,
        override_path: None,
    };
    let connection =
        McpServerConnection::with_agent_dir(entry, std::env::temp_dir().to_string_lossy(), std::env::temp_dir().to_string_lossy());
    connection.get_client().await.expect("connect over SSE");
    let tools = connection.tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let mut arguments = serde_json::Map::new();
    arguments.insert("text".to_string(), serde_json::json!("hello"));
    let result = connection
        .call_tool("echo", arguments, None)
        .await
        .expect("call tool");
    match &result.content[0] {
        pi_mcp::ContentBlock::Text { text, .. } => assert_eq!(text, "echo:hello"),
        other => panic!("expected text content, got {other:?}"),
    }
    connection.close().await;
}

/// Config file → runtime → connect: the whole path `create_agent_session` uses.
#[tokio::test]
async fn config_file_sse_server_is_connected() {
    let url = spawn_mock_sse_server().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path();
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(cwd.join(".pi-rs")).expect("mkdir");
    std::fs::write(
        cwd.join(".pi-rs").join("mcp.json"),
        format!(r#"{{"mcpServers":{{"mock":{{"type":"sse","url":"{url}"}}}}}}"#),
    )
    .expect("write mcp.json");

    let loaded = load_mcp_config(
        cwd.to_str().unwrap(),
        agent_dir.to_str().unwrap(),
    );
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(loaded.servers.len(), 1);
    let entry = loaded.servers[0].clone();
    let connection = McpServerConnection::with_agent_dir(
        entry,
        cwd.to_string_lossy(),
        agent_dir.to_string_lossy(),
    );
    connection.get_client().await.expect("connect configured SSE server");
    assert_eq!(connection.tools().len(), 1);
    assert_eq!(connection.tools()[0].name, "echo");
    connection.close().await;
}
