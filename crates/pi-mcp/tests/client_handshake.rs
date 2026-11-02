//! End-to-end client handshake over the in-memory transport.
//!
//! This exercises the same request/response, notification, and tool-call paths
//! the stdio and HTTP transports feed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use pi_mcp::protocol::jsonrpc::JsonRpcMessage;
use pi_mcp::protocol::types::LATEST_PROTOCOL_VERSION;
use pi_mcp::transports::in_memory::create_in_memory_transport_pair;
use pi_mcp::{
    McpClient, McpClientOptions, McpError, McpRequestOptions, McpTransport, McpTransportSendOptions,
};
use serde_json::{json, Value};

async fn run_server(transport: Arc<pi_mcp::transports::in_memory::InMemoryTransport>) {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _subscription = transport.on_message(Arc::new(move |message| {
        let _ = sender.send(message);
    }));
    transport.start().await.expect("server transport starts");
    while let Some(message) = receiver.recv().await {
        let JsonRpcMessage::Request(request) = message else {
            continue;
        };
        let result = match request.method.as_str() {
            "initialize" => json!({
                "protocolVersion": LATEST_PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": true}, "resources": {}},
                "serverInfo": {"name": "test", "version": "1"}
            }),
            "tools/list" => json!({
                "tools": [{
                    "name": "echo",
                    "description": "Echo text",
                    "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}
                }]
            }),
            "tools/call" => {
                let name = request
                    .params
                    .as_ref()
                    .and_then(|params| params.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if name == "echo" {
                    let text = request
                        .params
                        .as_ref()
                        .and_then(|params| params.get("arguments"))
                        .and_then(|arguments| arguments.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    json!({
                        "content": [{"type": "text", "text": format!("echo:{text}")}],
                        "isError": false
                    })
                } else {
                    json!({"content": [], "isError": true})
                }
            }
            other => {
                let response = JsonRpcMessage::error(
                    request.id,
                    pi_mcp::protocol::jsonrpc::JsonRpcErrorObject {
                        code: -32601,
                        message: format!("Method not found: {other}"),
                        data: None,
                    },
                );
                transport
                    .send(response, McpTransportSendOptions::default())
                    .await
                    .expect("server sends method-not-found");
                continue;
            }
        };
        transport
            .send(
                JsonRpcMessage::success(request.id, result),
                McpTransportSendOptions::default(),
            )
            .await
            .expect("server sends response");
    }
}

#[tokio::test]
async fn handshake_lists_and_calls_tools() {
    let (client_transport, server_transport) = create_in_memory_transport_pair();
    server_transport.start().await.expect("server transport starts");
    let server = tokio::spawn(run_server(server_transport));
    let client = McpClient::new(McpClientOptions::new("test", "1"));
    let initialized = client
        .connect(client_transport)
        .await
        .expect("client initializes");
    assert_eq!(initialized.protocol_version, LATEST_PROTOCOL_VERSION);
    assert_eq!(client.connection_state(), pi_mcp::client::ClientState::Connected);

    let tools = client
        .list_tools(McpRequestOptions::default())
        .await
        .expect("tools/list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let mut arguments = serde_json::Map::new();
    arguments.insert("text".to_string(), Value::String("hi".to_string()));
    let result = client
        .call_tool("echo", Some(arguments), McpRequestOptions::default())
        .await
        .expect("tools/call");
    assert!(!result.is_error);
    match &result.content[0] {
        pi_mcp::ContentBlock::Text { text, .. } => assert_eq!(text, "echo:hi"),
        other => panic!("expected text content, got {other:?}"),
    }

    client.close().await.expect("client closes");
    server.abort();
}

#[tokio::test]
async fn unknown_method_is_a_json_rpc_error() {
    let (client_transport, server_transport) = create_in_memory_transport_pair();
    server_transport.start().await.expect("server transport starts");
    let server = tokio::spawn(run_server(server_transport));
    let client = McpClient::new(McpClientOptions::new("test", "1"));
    client
        .connect(client_transport)
        .await
        .expect("client initializes");
    let error = client
        .request("nope", None, McpRequestOptions::default())
        .await
        .expect_err("unknown method fails");
    assert!(matches!(error, McpError::JsonRpc { code: -32601, .. }));
    client.close().await.expect("client closes");
    server.abort();
}
