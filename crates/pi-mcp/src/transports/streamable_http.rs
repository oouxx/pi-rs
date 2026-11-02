//! Streamable HTTP transport with SSE response/notification streams.
//!
//! Port of `packages/mcp/src/transports/streamable-http.ts`.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE,
};
use reqwest::{Method, Response, StatusCode};
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthProvider, UnauthorizedContext};
use crate::error::{McpError, Result};
use crate::protocol::jsonrpc::{
    parse_json_rpc_message, JsonRpcId, JsonRpcMessage, JsonRpcResponse, JSON_RPC_ERROR_CODES,
};
use crate::transport::{
    CloseListener, ErrorListener, McpTransport, McpTransportSendOptions, MessageListener,
    Subscription, TransportEvents, DEFAULT_MAX_MESSAGE_BYTES,
};

const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
const ERROR_MESSAGE_BODY_CHARS: usize = 500;
const DEFAULT_RECONNECT_INITIAL_DELAY_MS: u64 = 1_000;
const DEFAULT_RECONNECT_MAX_DELAY_MS: u64 = 30_000;
const DEFAULT_RECONNECT_MAX_RETRIES: usize = 5;

type ByteStream = Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>;

/// Reconnection settings for dropped SSE streams.
#[derive(Debug, Clone)]
pub struct StreamableHttpReconnectOptions {
    /// Delay before the first reconnection attempt.
    pub initial_delay_ms: u64,
    /// Upper bound for exponential backoff.
    pub max_delay_ms: u64,
    /// Consecutive failed attempts before giving up.
    pub max_retries: usize,
}

impl Default for StreamableHttpReconnectOptions {
    fn default() -> Self {
        Self {
            initial_delay_ms: DEFAULT_RECONNECT_INITIAL_DELAY_MS,
            max_delay_ms: DEFAULT_RECONNECT_MAX_DELAY_MS,
            max_retries: DEFAULT_RECONNECT_MAX_RETRIES,
        }
    }
}

/// Options for [`StreamableHttpTransport`].
#[derive(Clone)]
pub struct StreamableHttpTransportOptions {
    /// MCP server URL.
    pub url: url::Url,
    /// Extra HTTP headers.
    pub headers: BTreeMap<String, String>,
    /// Open the server-to-client GET stream after initialization.
    pub open_get_stream: bool,
    /// Maximum JSON-RPC message / SSE event size.
    pub max_message_bytes: usize,
    /// Reconnection settings.
    pub reconnect: StreamableHttpReconnectOptions,
    /// OAuth bearer token provider.
    pub auth: Option<Arc<dyn AuthProvider>>,
    /// HTTP client; a default client is created when absent.
    pub http: Option<reqwest::Client>,
}

impl StreamableHttpTransportOptions {
    /// Options for a URL with defaults.
    pub fn new(url: url::Url) -> Self {
        Self {
            url,
            headers: BTreeMap::new(),
            open_get_stream: true,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            reconnect: StreamableHttpReconnectOptions::default(),
            auth: None,
            http: None,
        }
    }
}

impl std::fmt::Debug for StreamableHttpTransportOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamableHttpTransportOptions")
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("open_get_stream", &self.open_get_stream)
            .field("reconnect", &self.reconnect)
            .finish()
    }
}

struct Inner {
    options: StreamableHttpTransportOptions,
    http: reqwest::Client,
    auth: Option<Arc<dyn AuthProvider>>,
    events: TransportEvents,
    controller: CancellationToken,
    started: AtomicBool,
    closed: AtomicBool,
    session_id: StdMutex<Option<String>>,
    protocol_version: StdMutex<Option<String>>,
    get_stream_started: AtomicBool,
    last_token: StdMutex<Option<String>>,
}

/// A streamable HTTP MCP transport.
#[derive(Clone)]
pub struct StreamableHttpTransport {
    inner: Arc<Inner>,
}

impl StreamableHttpTransport {
    /// Create a transport.
    pub fn new(options: StreamableHttpTransportOptions) -> Arc<Self> {
        let http = options.http.clone().unwrap_or_default();
        let auth = options.auth.clone();
        Arc::new(Self {
            inner: Arc::new(Inner {
                options,
                http,
                auth,
                events: TransportEvents::new(),
                controller: CancellationToken::new(),
                started: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                session_id: StdMutex::new(None),
                protocol_version: StdMutex::new(None),
                get_stream_started: AtomicBool::new(false),
                last_token: StdMutex::new(None),
            }),
        })
    }

    /// Current MCP session id.
    pub fn session_id(&self) -> Option<String> {
        self.inner
            .session_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    fn set_last_token(&self, token: Option<String>) {
        *self
            .inner
            .last_token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = token;
    }

    fn last_token(&self) -> Option<String> {
        self.inner
            .last_token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn capture_session(&self, response: &Response) {
        if let Some(value) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *self
                .inner
                .session_id
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value.to_string());
        }
    }

    fn build_headers(&self, extra: Option<&HeaderMap>, token: Option<&str>) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        for (name, value) in &self.inner.options.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| McpError::Transport(format!("invalid header name '{name}': {error}")))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|error| McpError::Transport(format!("invalid header value for '{name}': {error}")))?;
            headers.insert(header_name, header_value);
        }
        if let Some(extra) = extra {
            for (name, value) in extra {
                headers.insert(name.clone(), value.clone());
            }
        }
        if let Some(session_id) = self.session_id() {
            headers.insert(
                "Mcp-Session-Id",
                HeaderValue::from_str(&session_id)
                    .map_err(|error| McpError::Transport(error.to_string()))?,
            );
        }
        if let Some(version) = self
            .inner
            .protocol_version
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            headers.insert(
                "MCP-Protocol-Version",
                HeaderValue::from_str(&version)
                    .map_err(|error| McpError::Transport(error.to_string()))?,
            );
        }
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|error| McpError::Transport(error.to_string()))?,
            );
        }
        Ok(headers)
    }

    async fn authorized_fetch(
        &self,
        method: Method,
        extra_headers: Option<&HeaderMap>,
        body: Option<String>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Response> {
        let mut attempt = 0usize;
        loop {
            let token = match &self.inner.auth {
                Some(auth) => auth.token().await,
                None => None,
            };
            self.set_last_token(token.clone());
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(McpError::Aborted);
            }
            let headers = self.build_headers(extra_headers, token.as_deref())?;
            let mut request = self
                .inner
                .http
                .request(method.clone(), self.inner.options.url.clone())
                .headers(headers);
            if let Some(body) = &body {
                request = request.body(body.clone());
            }
            let response = tokio::select! {
                () = self.inner.controller.cancelled() => {
                    return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
                }
                () = maybe_cancelled(cancel) => return Err(McpError::Aborted),
                response = request.send() => response?,
            };
            if attempt > 0 || self.inner.auth.is_none() || !needs_authorization(&response) {
                return Ok(response);
            }
            let Some(auth) = &self.inner.auth else {
                return Ok(response);
            };
            let www_authenticate = response
                .headers()
                .get("www-authenticate")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let context = UnauthorizedContext {
                status: response.status().as_u16(),
                www_authenticate,
                server_url: self.inner.options.url.clone(),
                token,
                http: self.inner.http.clone(),
            };
            drop(response);
            auth.on_unauthorized(context).await?;
            attempt += 1;
        }
    }

    async fn check_response(&self, response: Response) -> Result<Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let www_authenticate = response
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(MAX_ERROR_BODY_BYTES)
            .collect::<String>();
        if status == 401 {
            return Err(McpError::AuthRequired {
                www_authenticate,
                body,
            });
        }
        if status == 404 && self.session_id().is_some() {
            return Err(McpError::SessionExpired { body });
        }
        let text = body.trim();
        let snippet = if text.chars().count() > ERROR_MESSAGE_BODY_CHARS {
            format!(
                "{}...",
                text.chars()
                    .take(ERROR_MESSAGE_BODY_CHARS - 3)
                    .collect::<String>()
            )
        } else {
            text.to_string()
        };
        Err(McpError::Http {
            status,
            body,
            display: if snippet.is_empty() {
                String::new()
            } else {
                format!(": {snippet}")
            },
        })
    }

    fn handle_sse_event(
        &self,
        cursor: &mut StreamCursor,
        event: SseEvent,
        on_message: &mut (dyn FnMut(&JsonRpcMessage) + Send),
    ) {
        cursor.received = true;
        if let Some(retry_ms) = event.retry_ms {
            cursor.retry_ms = Some(retry_ms);
        }
        if let Some(id) = &event.id {
            cursor.last_event_id = Some(id.clone());
        }
        if event.data.trim().is_empty()
            || event.event.as_deref().is_some_and(|name| name != "message")
        {
            return;
        }
        match serde_json::from_str::<serde_json::Value>(&event.data)
            .map_err(McpError::from)
            .and_then(parse_json_rpc_message)
        {
            Ok(message) => {
                on_message(&message);
                self.inner.events.emit_message(message);
            }
            Err(error) => self.inner.events.emit_error(&error),
        }
    }

    async fn consume_sse(
        &self,
        stream: ByteStream,
        cursor: &mut StreamCursor,
        on_message: &mut (dyn FnMut(&JsonRpcMessage) + Send),
    ) -> Result<()> {
        let max = self.inner.options.max_message_bytes;
        let mut parser = SseParser::new(max);
        let mut stream = stream;
        let mut handle = |event: SseEvent| self.handle_sse_event(cursor, event, on_message);
        loop {
            let chunk = tokio::select! {
                () = self.inner.controller.cancelled() => {
                    return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
                }
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(McpError::from)?;
            parser.feed(&chunk, &mut handle)?;
        }
        parser.finish(&mut handle)?;
        Ok(())
    }

    async fn consume_response_stream(
        self: Arc<Self>,
        stream: ByteStream,
        request_id: JsonRpcId,
    ) {
        let mut cursor = StreamCursor::default();
        let answered = Arc::new(AtomicBool::new(false));
        let answered_flag = Arc::clone(&answered);
        let compare_id = request_id.clone();
        let on_message = move |message: &JsonRpcMessage| {
            if let JsonRpcMessage::Response(JsonRpcResponse::Success { id, .. })
            | JsonRpcMessage::Response(JsonRpcResponse::Error { id, .. }) = message
            {
                if *id == compare_id {
                    answered_flag.store(true, Ordering::SeqCst);
                }
            }
        };
        let mut on_message = on_message;
        let mut next: Option<ByteStream> = Some(stream);
        let mut failure: Option<McpError> = None;
        let mut attempt = 0usize;
        loop {
            if let Some(stream) = next.take() {
                match self.consume_sse(stream, &mut cursor, &mut on_message).await {
                    Ok(()) => failure = None,
                    Err(error) => failure = Some(error),
                }
            }
            if answered.load(Ordering::SeqCst) || self.is_closed() {
                return;
            }
            if failure.as_ref().is_some_and(|error| !self.is_retryable(error)) {
                break;
            }
            if cursor.last_event_id.is_none() || attempt >= self.max_retries() {
                break;
            }
            if cursor.received {
                attempt = 0;
            }
            cursor.received = false;
            if !self.sleep(self.reconnect_delay(attempt, cursor.retry_ms)).await {
                return;
            }
            attempt += 1;
            match self.open_sse_stream(cursor.last_event_id.clone()).await {
                Ok(Some(stream)) => next = Some(stream),
                Ok(None) => {
                    failure = Some(McpError::Transport(
                        "MCP response stream closed without a response".to_string(),
                    ));
                    break;
                }
                Err(error) => {
                    if !self.is_retryable(&error) {
                        failure = Some(error);
                        break;
                    }
                    failure = Some(error);
                }
            }
        }
        if self.is_closed() {
            return;
        }
        let reason = failure
            .map(|error| error.to_string())
            .unwrap_or_else(|| "stream ended without a response".to_string());
        self.inner.events.emit_message(JsonRpcMessage::error(
            request_id,
            crate::protocol::jsonrpc::JsonRpcErrorObject {
                code: JSON_RPC_ERROR_CODES.internal_error,
                message: format!("MCP response stream failed: {reason}"),
                data: None,
            },
        ));
    }

    async fn run_get_stream(self: Arc<Self>) {
        let mut cursor = StreamCursor::default();
        let mut attempt = 0usize;
        while !self.is_closed() {
            let opened_at = std::time::Instant::now();
            match self.open_sse_stream(cursor.last_event_id.clone()).await {
                Ok(Some(stream)) => {
                    let mut noop = |_: &JsonRpcMessage| {};
                    let result = self.consume_sse(stream, &mut cursor, &mut noop).await;
                    if cursor.received || opened_at.elapsed() > self.max_delay() {
                        attempt = 0;
                    }
                    if let Err(error) = result {
                        if self.is_closed() {
                            return;
                        }
                        if !self.is_retryable(&error) {
                            self.inner.events.emit_error(&error);
                            return;
                        }
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    if self.is_closed() {
                        return;
                    }
                    if !self.is_retryable(&error) {
                        self.inner.events.emit_error(&error);
                        return;
                    }
                }
            }
            cursor.received = false;
            if attempt >= self.max_retries() {
                self.inner.events.emit_error(&McpError::Transport(
                    "MCP server-to-client stream dropped and could not be reopened".to_string(),
                ));
                return;
            }
            let delay = self.reconnect_delay(attempt, cursor.retry_ms);
            attempt += 1;
            if !self.sleep(delay).await {
                return;
            }
        }
    }

    async fn open_sse_stream(&self, last_event_id: Option<String>) -> Result<Option<ByteStream>> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        if let Some(last_event_id) = last_event_id {
            headers.insert(
                HeaderName::from_static("last-event-id"),
                HeaderValue::from_str(&last_event_id)
                    .map_err(|error| McpError::Transport(error.to_string()))?,
            );
        }
        let response = self
            .authorized_fetch(Method::GET, Some(&headers), None, None)
            .await?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(None);
        }
        let response = self.check_response(response).await?;
        self.capture_session(&response);
        let content_type = content_type(&response);
        if content_type.as_deref() != Some("text/event-stream") {
            return Err(McpError::Http {
                status: response.status().as_u16(),
                body: String::new(),
                display: format!(
                    ": Unsupported MCP GET response content type: {}",
                    content_type.as_deref().unwrap_or("missing")
                ),
            });
        }
        Ok(Some(Box::pin(response.bytes_stream())))
    }

    fn is_retryable(&self, error: &McpError) -> bool {
        match error {
            McpError::Http { status, .. } => {
                *status == 408 || *status == 429 || (*status >= 500 && *status != 501)
            }
            McpError::Network(_) | McpError::Timeout(_) | McpError::Transport(_) => true,
            _ => false,
        }
    }

    fn reconnect_delay(&self, attempt: usize, server_delay_ms: Option<u64>) -> Duration {
        if let Some(delay) = server_delay_ms {
            return Duration::from_millis(delay);
        }
        let initial = self.inner.options.reconnect.initial_delay_ms;
        let delay = initial.saturating_mul(2u64.saturating_pow(attempt.min(16) as u32));
        Duration::from_millis(delay.min(self.max_delay().as_millis() as u64))
    }

    fn max_delay(&self) -> Duration {
        Duration::from_millis(self.inner.options.reconnect.max_delay_ms)
    }

    fn max_retries(&self) -> usize {
        self.inner.options.reconnect.max_retries
    }

    /// Resolves false when the transport closed while waiting.
    async fn sleep(&self, duration: Duration) -> bool {
        tokio::select! {
            () = self.inner.controller.cancelled() => false,
            () = tokio::time::sleep(duration) => true,
        }
    }
}

async fn maybe_cancelled(cancel: Option<&CancellationToken>) {
    match cancel {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

fn needs_authorization(response: &Response) -> bool {
    if response.status() == StatusCode::UNAUTHORIZED {
        return true;
    }
    if response.status() != StatusCode::FORBIDDEN {
        return false;
    }
    response
        .headers()
        .get("www-authenticate")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("insufficient_scope"))
}

fn content_type(response: &Response) -> Option<String> {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn start(&self) -> Result<()> {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return Err(McpError::Transport(
                "MCP Streamable HTTP transport already started".to_string(),
            ));
        }
        if self.is_closed() {
            self.inner.started.store(false, Ordering::SeqCst);
            return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
        }
        Ok(())
    }

    fn set_protocol_version(&self, version: &str) {
        *self
            .inner
            .protocol_version
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(version.to_string());
    }

    async fn send(&self, message: JsonRpcMessage, options: McpTransportSendOptions) -> Result<()> {
        if !self.inner.started.load(Ordering::SeqCst) || self.is_closed() {
            return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
        }
        let is_request = matches!(message, JsonRpcMessage::Request(_));
        let request_id = match &message {
            JsonRpcMessage::Request(request) => Some(request.id.clone()),
            _ => None,
        };
        let method = message.method().map(str::to_string);
        let payload = message.to_json_string()?;
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = self
            .authorized_fetch(
                Method::POST,
                Some(&headers),
                Some(payload),
                options.cancel.as_ref(),
            )
            .await?;
        let response = self.check_response(response).await?;
        self.capture_session(&response);

        if !is_request {
            drop(response);
            if method.as_deref() == Some("notifications/initialized")
                && self.inner.options.open_get_stream
                && !self.is_closed()
                && !self.inner.get_stream_started.swap(true, Ordering::SeqCst)
            {
                let transport = self.clone_arc();
                tokio::spawn(async move {
                    transport.run_get_stream().await;
                });
            }
            return Ok(());
        }

        let status = response.status().as_u16();
        if status == 202 || status == 204 {
            return Err(McpError::Http {
                status,
                body: String::new(),
                display: format!(
                    ": MCP server accepted request {} without a response",
                    method.unwrap_or_default()
                ),
            });
        }
        let content_type = content_type(&response);
        match content_type.as_deref() {
            Some("application/json") => {
                let body: serde_json::Value = response.json().await?;
                let values = match body {
                    serde_json::Value::Array(values) => values,
                    value => vec![value],
                };
                for value in values {
                    self.inner.events.emit_message(parse_json_rpc_message(value)?);
                }
            }
            Some("text/event-stream") => {
                let stream: ByteStream = Box::pin(response.bytes_stream());
                let transport = self.clone_arc();
                let request_id = request_id
                    .ok_or_else(|| McpError::Transport("missing MCP request id".to_string()))?;
                tokio::spawn(async move {
                    transport.consume_response_stream(stream, request_id).await;
                });
            }
            other => {
                return Err(McpError::Http {
                    status,
                    body: String::new(),
                    display: format!(
                        ": Unsupported MCP response content type: {}",
                        other.unwrap_or("missing")
                    ),
                })
            }
        }
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.inner.controller.cancel();
        if self.inner.started.load(Ordering::SeqCst) {
            if let Some(_session_id) = self.session_id() {
                let headers = self.build_headers(None, self.last_token().as_deref())?;
                let request = self
                    .inner
                    .http
                    .request(Method::DELETE, self.inner.options.url.clone())
                    .headers(headers);
                let _ = tokio::time::timeout(Duration::from_secs(1), request.send()).await;
            }
        }
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

impl StreamableHttpTransport {
    /// Clone the transport as an owned `Arc` for spawned tasks.
    ///
    /// The transport methods receive `&self` through `Arc<dyn McpTransport>`;
    /// this helper reconstructs an `Arc` from the inner pointer's ownership
    /// model by keeping an `Arc` in the transport itself.
    fn clone_arc(&self) -> Arc<Self> {
        // The struct is only ever created as `Arc<Self>` (see `new`). Cloning
        // the `Arc<Inner>` and wrapping it in a new `Arc<Self>` is safe and
        // cheap; both share the same inner state.
        Arc::new(Self {
            inner: Arc::clone(&self.inner),
        })
    }
}

#[derive(Default)]
struct StreamCursor {
    last_event_id: Option<String>,
    retry_ms: Option<u64>,
    received: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SseEvent {
    event: Option<String>,
    data: String,
    id: Option<String>,
    retry_ms: Option<u64>,
}

struct SseParser {
    max_event_bytes: usize,
    buffer: Vec<u8>,
    data_lines: Vec<String>,
    data_bytes: usize,
    event: Option<String>,
    id: Option<String>,
    retry_ms: Option<u64>,
}

impl SseParser {
    fn new(max_event_bytes: usize) -> Self {
        Self {
            max_event_bytes,
            buffer: Vec::new(),
            data_lines: Vec::new(),
            data_bytes: 0,
            event: None,
            id: None,
            retry_ms: None,
        }
    }

    fn feed(&mut self, chunk: &[u8], on_event: &mut (dyn FnMut(SseEvent) + Send)) -> Result<()> {
        self.buffer.extend_from_slice(chunk);
        loop {
            let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') else {
                if self.buffer.len() > self.max_event_bytes {
                    return Err(McpError::Transport(format!(
                        "MCP SSE event exceeds {} bytes",
                        self.max_event_bytes
                    )));
                }
                return Ok(());
            };
            let mut line = self.buffer.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line)
                .map_err(|_| McpError::Transport("MCP SSE stream is not UTF-8".to_string()))?;
            self.process_line(line, on_event)?;
        }
    }

    fn finish(&mut self, on_event: &mut (dyn FnMut(SseEvent) + Send)) -> Result<()> {
        if !self.buffer.is_empty() {
            let mut line = std::mem::take(&mut self.buffer);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line)
                .map_err(|_| McpError::Transport("MCP SSE stream is not UTF-8".to_string()))?;
            self.process_line(line, on_event)?;
        }
        self.dispatch(on_event);
        Ok(())
    }

    fn process_line(&mut self, line: &str, on_event: &mut (dyn FnMut(SseEvent) + Send)) -> Result<()> {
        if line.is_empty() {
            self.dispatch(on_event);
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = match line.find(':') {
            Some(index) => (&line[..index], &line[index + 1..]),
            None => (line, ""),
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                self.data_bytes += value.len() + usize::from(!self.data_lines.is_empty());
                if self.data_bytes > self.max_event_bytes {
                    return Err(McpError::Transport(format!(
                        "MCP SSE event exceeds {} bytes",
                        self.max_event_bytes
                    )));
                }
                self.data_lines.push(value.to_string());
            }
            "event" => self.event = Some(value.to_string()),
            "id" => {
                if !value.contains('\0') {
                    self.id = Some(value.to_string());
                }
            }
            "retry" if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) => {
                self.retry_ms = value.parse::<u64>().ok();
            }
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, on_event: &mut (dyn FnMut(SseEvent) + Send)) {
        if self.data_lines.is_empty() {
            self.event = None;
            self.id = None;
            return;
        }
        let data = self.data_lines.join("\n");
        on_event(SseEvent {
            event: self.event.take(),
            data,
            id: self.id.take(),
            retry_ms: self.retry_ms.take(),
        });
        self.data_lines.clear();
        self.data_bytes = 0;
    }
}
