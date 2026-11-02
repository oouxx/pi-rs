//! Legacy HTTP+SSE transport (protocol revision 2024-11-05).
//!
//! The TS v1.0.0 client removed this transport; pi-rs keeps it as an
//! intentional deviation (see `DEVIATIONS.md` #14). The server opens an SSE
//! stream, sends an `endpoint` event, and the client POSTs JSON-RPC messages
//! to that endpoint.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::{Client, Method};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error::{McpError, Result};
use crate::protocol::jsonrpc::{parse_json_rpc_message, JsonRpcMessage};
use crate::transport::{
    CloseListener, ErrorListener, McpTransport, McpTransportSendOptions, MessageListener,
    Subscription, TransportEvents, DEFAULT_MAX_MESSAGE_BYTES,
};

const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);

/// Options for [`LegacySseTransport`].
#[derive(Clone)]
pub struct LegacySseTransportOptions {
    /// SSE stream URL.
    pub url: Url,
    /// Extra HTTP headers.
    pub headers: BTreeMap<String, String>,
    /// HTTP client.
    pub http: Option<Client>,
    /// Maximum SSE event / JSON-RPC message size.
    pub max_message_bytes: usize,
}

impl LegacySseTransportOptions {
    /// Options for a URL.
    pub fn new(url: Url) -> Self {
        Self {
            url,
            headers: BTreeMap::new(),
            http: None,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }
}

impl std::fmt::Debug for LegacySseTransportOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacySseTransportOptions")
            .field("url", &self.url)
            .finish()
    }
}

struct Inner {
    options: LegacySseTransportOptions,
    http: Client,
    events: TransportEvents,
    controller: CancellationToken,
    endpoint: Mutex<Option<Url>>,
    endpoint_ready: Notify,
    started: AtomicBool,
    closed: AtomicBool,
}

/// A legacy SSE client transport.
#[derive(Clone)]
pub struct LegacySseTransport {
    inner: Arc<Inner>,
}

impl LegacySseTransport {
    /// Create a transport.
    pub fn new(options: LegacySseTransportOptions) -> Arc<Self> {
        let http = options.http.clone().unwrap_or_default();
        Arc::new(Self {
            inner: Arc::new(Inner {
                options,
                http,
                events: TransportEvents::new(),
                controller: CancellationToken::new(),
                endpoint: Mutex::new(None),
                endpoint_ready: Notify::new(),
                started: AtomicBool::new(false),
                closed: AtomicBool::new(false),
            }),
        })
    }

    fn build_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        for (name, value) in &self.inner.options.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| McpError::Transport(format!("invalid header name '{name}': {error}")))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|error| McpError::Transport(format!("invalid header value for '{name}': {error}")))?;
            headers.insert(header_name, header_value);
        }
        Ok(headers)
    }

    fn endpoint(&self) -> Option<Url> {
        self.inner
            .endpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_endpoint(&self, endpoint: Url) {
        *self
            .inner
            .endpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(endpoint);
        self.inner.endpoint_ready.notify_waiters();
    }

    fn handle_sse_event(&self, event: Option<&str>, data: &str) {
        if event == Some("endpoint") {
            let data = data.trim();
            if data.is_empty() {
                return;
            }
            let endpoint = match Url::parse(data) {
                Ok(url) => url,
                Err(_) => match self.inner.options.url.join(data) {
                    Ok(url) => url,
                    Err(error) => {
                        self.inner.events.emit_error(&McpError::Transport(format!(
                            "invalid MCP SSE endpoint: {error}"
                        )));
                        return;
                    }
                },
            };
            self.set_endpoint(endpoint);
            return;
        }
        if event.is_some_and(|event| event != "message") {
            return;
        }
        if data.trim().is_empty() {
            return;
        }
        match serde_json::from_str::<serde_json::Value>(data)
            .map_err(McpError::from)
            .and_then(parse_json_rpc_message)
        {
            Ok(message) => self.inner.events.emit_message(message),
            Err(error) => self.inner.events.emit_error(&error),
        }
    }

    async fn read_stream(
        self: Arc<Self>,
        stream: Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>,
    ) {
        let max = self.inner.options.max_message_bytes;
        let mut buffer: Vec<u8> = Vec::new();
        let mut event: Option<String> = None;
        let mut data_lines: Vec<String> = Vec::new();
        let dispatch = |event: &mut Option<String>, data_lines: &mut Vec<String>, this: &Self| {
            if !data_lines.is_empty() {
                let data = data_lines.join("\n");
                this.handle_sse_event(event.as_deref(), &data);
            }
            *event = None;
            data_lines.clear();
        };
        let mut stream = stream;
        loop {
            let chunk = tokio::select! {
                () = self.inner.controller.cancelled() => break,
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = chunk else { break };
            let Ok(chunk) = chunk else { break };
            buffer.extend_from_slice(&chunk);
            loop {
                let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') else {
                    if buffer.len() > max {
                        self.inner.events.emit_error(&McpError::Transport(format!(
                            "MCP SSE event exceeds {max} bytes"
                        )));
                        buffer.clear();
                    }
                    break;
                };
                let mut line = buffer.drain(..=newline).collect::<Vec<_>>();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let Ok(line) = std::str::from_utf8(&line) else {
                    continue;
                };
                if line.is_empty() {
                    dispatch(&mut event, &mut data_lines, &self);
                    continue;
                }
                if line.starts_with(':') {
                    continue;
                }
                let (field, value) = match line.find(':') {
                    Some(index) => (&line[..index], &line[index + 1..]),
                    None => (line, ""),
                };
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "event" => event = Some(value.to_string()),
                    "data" => data_lines.push(value.to_string()),
                    _ => {}
                }
            }
        }
        dispatch(&mut event, &mut data_lines, &self);
        if !self.inner.closed.load(Ordering::SeqCst) {
            self.inner.events.emit_close();
        }
    }
}

#[async_trait]
impl McpTransport for LegacySseTransport {
    async fn start(&self) -> Result<()> {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return Err(McpError::Transport(
                "MCP SSE transport already started".to_string(),
            ));
        }
        if self.inner.closed.load(Ordering::SeqCst) {
            self.inner.started.store(false, Ordering::SeqCst);
            return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
        }
        let mut headers = self.build_headers()?;
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        let request = self
            .inner
            .http
            .request(Method::GET, self.inner.options.url.clone())
            .headers(headers);
        let response = tokio::time::timeout(ENDPOINT_TIMEOUT, request.send())
            .await
            .map_err(|_| McpError::Timeout(ENDPOINT_TIMEOUT))??;
        if !response.status().is_success() {
            return Err(McpError::Http {
                status: response.status().as_u16(),
                body: String::new(),
                display: String::new(),
            });
        }
        let stream: Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>> =
            Box::pin(response.bytes_stream());
        let transport = Arc::new(self.clone());
        tokio::spawn(async move {
            transport.read_stream(stream).await;
        });
        // Wait for the endpoint event.
        tokio::time::timeout(ENDPOINT_TIMEOUT, async {
            loop {
                if self.endpoint().is_some() {
                    return;
                }
                self.inner.endpoint_ready.notified().await;
            }
        })
        .await
        .map_err(|_| McpError::Timeout(ENDPOINT_TIMEOUT))?;
        Ok(())
    }

    async fn send(&self, message: JsonRpcMessage, options: McpTransportSendOptions) -> Result<()> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
        }
        if options.is_cancelled() {
            return Err(McpError::Aborted);
        }
        let Some(endpoint) = self.endpoint() else {
            return Err(McpError::ConnectionClosed(
                "MCP SSE endpoint is not ready".to_string(),
            ));
        };
        let payload = message.to_json_string()?;
        let mut headers = self.build_headers()?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = self
            .inner
            .http
            .post(endpoint)
            .headers(headers)
            .body(payload)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(McpError::Http {
                status: response.status().as_u16(),
                body: String::new(),
                display: String::new(),
            });
        }
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.inner.controller.cancel();
        self.inner.events.emit_close();
        Ok(())
    }

    fn on_message(&self, listener: MessageListener) -> Subscription {
        self.inner.events.on_message(listener)
    }

    fn on_error(&self, listener: ErrorListener) -> Subscription {
        self.inner.events.on_error(listener)
    }

    fn on_close(&self, listener: CloseListener) -> Subscription {
        self.inner.events.on_close(listener)
    }
}
