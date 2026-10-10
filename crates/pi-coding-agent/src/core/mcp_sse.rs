//! Legacy MCP SSE client transport (protocol revision 2024-11-05).
//!
//! rmcp 1.8 ships only stdio and streamable-HTTP client transports, so pi
//! implements the legacy SSE transport on top of rmcp's [`Transport`] trait:
//!
//! 1. `GET` the SSE URL with `Accept: text/event-stream`.
//! 2. The server's first event is `event: endpoint` whose data is the URL the
//!    client POSTs JSON-RPC messages to (relative URLs resolve against the SSE
//!    URL).
//! 3. Every other event carries a JSON-RPC message for the client.
//!
//! The reader task is spawned by [`SseClientTransport::connect`], which waits
//! for the endpoint event before returning so [`Transport::send`] never races
//! the handshake.
//!
//! This module is feature-gated behind `mcp` (default-on).

use std::collections::BTreeMap;
use std::time::Duration;

use futures::StreamExt;
use rmcp::model::ServerJsonRpcMessage;
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use rmcp::RoleClient;
use tokio::sync::{mpsc, oneshot};

/// How long [`SseClientTransport::connect`] waits for the `endpoint` event.
const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);

/// Errors of the legacy SSE transport.
#[derive(Debug, thiserror::Error)]
pub enum SseTransportError {
    /// The initial `GET` or a POST failed at the HTTP level.
    #[error("SSE HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The server answered the initial `GET` with a non-success status.
    #[error("SSE server returned HTTP {0}")]
    Status(u16),
    /// The stream closed (or timed out) before the `endpoint` event arrived.
    #[error("SSE stream closed before the endpoint event")]
    MissingEndpoint,
    /// The `endpoint` event data could not be resolved against the SSE URL.
    #[error("invalid SSE endpoint event: {0}")]
    InvalidEndpoint(String),
    /// A POST to the message endpoint returned a non-success status.
    #[error("POST to the SSE message endpoint returned HTTP {0}")]
    PostStatus(u16),
    /// A JSON-RPC message could not be serialized/deserialized.
    #[error("invalid MCP JSON-RPC message: {0}")]
    Decode(String),
}

/// A connected legacy SSE transport.
pub struct SseClientTransport {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    headers: BTreeMap<String, String>,
    rx: mpsc::UnboundedReceiver<ServerJsonRpcMessage>,
    reader: tokio::task::JoinHandle<()>,
}

impl SseClientTransport {
    /// Open the SSE stream and wait for the `endpoint` event.
    pub async fn connect(
        url: &str,
        headers: BTreeMap<String, String>,
    ) -> Result<Self, SseTransportError> {
        let base = reqwest::Url::parse(url)
            .map_err(|e| SseTransportError::InvalidEndpoint(e.to_string()))?;
        let client = reqwest::Client::new();
        let mut request = client
            .get(base.clone())
            .header(reqwest::header::ACCEPT, "text/event-stream");
        for (name, value) in &headers {
            request = request.header(name, value);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(SseTransportError::Status(status.as_u16()));
        }

        let (endpoint_tx, endpoint_rx) = oneshot::channel();
        let (tx, rx) = mpsc::unbounded_channel();
        let mut stream = sse_stream::SseStream::from_bytes_stream(response.bytes_stream());
        let reader = tokio::spawn(async move {
            let mut endpoint_tx = Some(endpoint_tx);
            while let Some(event) = stream.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(_) => break,
                };
                if event.event.as_deref() == Some("endpoint") {
                    if let (Some(sender), Some(data)) = (endpoint_tx.take(), event.data.as_deref()) {
                        match base.join(data.trim()) {
                            Ok(resolved) => {
                                let _ = sender.send(Ok(resolved));
                            }
                            Err(e) => {
                                let _ = sender.send(Err(e.to_string()));
                            }
                        }
                    }
                    continue;
                }
                let Some(data) = event.data.as_deref() else {
                    continue;
                };
                match serde_json::from_str::<ServerJsonRpcMessage>(data) {
                    Ok(message) => {
                        if tx.send(message).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        // Surface parse failures instead of silently dropping.
                        eprintln!("[pi] MCP SSE: dropping unparsable message: {e}");
                    }
                }
            }
        });

        let endpoint = tokio::time::timeout(ENDPOINT_TIMEOUT, endpoint_rx)
            .await
            .map_err(|_| SseTransportError::MissingEndpoint)?
            .map_err(|_| SseTransportError::MissingEndpoint)?
            .map_err(SseTransportError::InvalidEndpoint)?;

        Ok(Self {
            client,
            endpoint,
            headers,
            rx,
            reader,
        })
    }
}

impl Transport<RoleClient> for SseClientTransport {
    type Error = SseTransportError;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let headers = self.headers.clone();
        async move {
            let body =
                serde_json::to_string(&item).map_err(|e| SseTransportError::Decode(e.to_string()))?;
            let mut request = client
                .post(endpoint)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
            for (name, value) in &headers {
                request = request.header(name, value);
            }
            let response = request.send().await?;
            let status = response.status();
            if !status.is_success() {
                return Err(SseTransportError::PostStatus(status.as_u16()));
            }
            Ok(())
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        self.rx.recv().await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.reader.abort();
        Ok(())
    }
}
