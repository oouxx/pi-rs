//! MCP client: request/response correlation, initialization, pagination, and
//! lifecycle.
//!
//! Port of `packages/mcp/src/client.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration as StdDuration;

use futures::future::BoxFuture;
use serde_json::{Map, Value};
use tokio::sync::{oneshot, Notify};
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

use crate::error::{McpError, Result};
use crate::protocol::content::CallToolResult;
use crate::protocol::jsonrpc::{
    JsonRpcErrorObject, JsonRpcId, JsonRpcMessage, JsonRpcResponse, JSON_RPC_ERROR_CODES,
};
use crate::protocol::types::{
    CancelledNotification, ClientCapabilities, Implementation, InitializeParams, InitializeResult,
    ListResourceTemplatesResult, ListResourcesResult, ProgressNotification, Resource,
    ResourceTemplate, Root, RootsCapability, ServerCapabilities, Tool, LATEST_PROTOCOL_VERSION,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use crate::transport::{McpTransport, McpTransportSendOptions, Subscription};

const DEFAULT_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const MAX_LIST_PAGES: usize = 1_000;

/// Connection state of an [`McpClient`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    /// Never connected.
    Idle,
    /// Transport started, `initialize` in flight.
    Connecting,
    /// Initialized.
    Connected,
    /// Closed.
    Closed,
}

impl std::fmt::Display for ClientState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Idle => "idle",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Closed => "closed",
        })
    }
}

/// A provider for `roots/list`.
pub enum RootsProvider {
    /// Fixed roots.
    Static(Vec<Root>),
    /// Callback that computes roots.
    Dynamic(Arc<dyn Fn() -> BoxFuture<'static, Vec<Root>> + Send + Sync>),
}

impl std::fmt::Debug for RootsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RootsProvider")
    }
}

impl Clone for RootsProvider {
    fn clone(&self) -> Self {
        match self {
            Self::Static(roots) => Self::Static(roots.clone()),
            Self::Dynamic(provider) => Self::Dynamic(Arc::clone(provider)),
        }
    }
}

/// Progress callback for one request.
pub type ProgressFn = Arc<dyn Fn(ProgressNotification) + Send + Sync>;
/// Handler for a server-to-client request.
pub type RequestHandler =
    Arc<dyn Fn(Option<Value>, CancellationToken) -> BoxFuture<'static, Result<Value>> + Send + Sync>;
/// Notification listener.
pub type NotificationListener = Arc<dyn Fn(Option<Value>) + Send + Sync>;
/// Error listener.
pub type ErrorListener = Arc<dyn Fn(&McpError) + Send + Sync>;
/// Close listener.
pub type CloseListener = Arc<dyn Fn() + Send + Sync>;

/// Options for [`McpClient`].
#[derive(Clone)]
pub struct McpClientOptions {
    /// Client name sent in `initialize`.
    pub name: String,
    /// Client version sent in `initialize`.
    pub version: String,
    /// Optional client title.
    pub title: Option<String>,
    /// Client capabilities.
    pub capabilities: ClientCapabilities,
    /// Protocol version to request; defaults to [`LATEST_PROTOCOL_VERSION`].
    pub protocol_version: Option<String>,
    /// Default request timeout.
    pub request_timeout: Option<StdDuration>,
    /// Roots exposed to the server.
    pub roots: Option<RootsProvider>,
}

impl McpClientOptions {
    /// Options with the given client name and version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            title: None,
            capabilities: ClientCapabilities::default(),
            protocol_version: None,
            request_timeout: None,
            roots: None,
        }
    }
}

impl std::fmt::Debug for McpClientOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClientOptions")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("title", &self.title)
            .field("protocol_version", &self.protocol_version)
            .field("request_timeout", &self.request_timeout)
            .field("roots", &self.roots.is_some())
            .finish()
    }
}

/// Options for one request.
#[derive(Clone, Default)]
pub struct McpRequestOptions {
    /// Cancellation token.
    pub cancel: Option<CancellationToken>,
    /// Timeout for this request.
    pub timeout: Option<StdDuration>,
    /// Progress callback. When set, the request carries a progress token.
    pub on_progress: Option<ProgressFn>,
}

impl std::fmt::Debug for McpRequestOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRequestOptions")
            .field("cancel", &self.cancel.is_some())
            .field("timeout", &self.timeout)
            .field("on_progress", &self.on_progress.is_some())
            .finish()
    }
}

struct PendingRequest {
    sender: oneshot::Sender<Result<Value>>,
    cancellable: bool,
    progress_token: Option<JsonRpcId>,
    on_progress: Option<ProgressFn>,
    timeout_reset: Arc<Notify>,
    send_cancel: CancellationToken,
}

type ListenerId = u64;
type NotificationEntry = (ListenerId, NotificationListener);
type ErrorEntry = (ListenerId, ErrorListener);
type CloseEntry = (ListenerId, CloseListener);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An MCP client.
///
/// Cheap to clone; all clones share the same connection.
#[derive(Clone)]
pub struct McpClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    options: McpClientOptions,
    state: Mutex<ClientState>,
    transport: RwLock<Option<Arc<dyn McpTransport>>>,
    next_request_id: AtomicI64,
    server_info: RwLock<Option<Implementation>>,
    server_capabilities: RwLock<Option<ServerCapabilities>>,
    instructions: RwLock<Option<String>>,
    protocol_version: RwLock<Option<String>>,
    pending: Mutex<HashMap<JsonRpcId, PendingRequest>>,
    progress_requests: Mutex<HashMap<JsonRpcId, JsonRpcId>>,
    incoming: Mutex<HashMap<JsonRpcId, CancellationToken>>,
    request_handlers: Arc<RwLock<HashMap<String, RequestHandler>>>,
    notification_listeners: Arc<RwLock<HashMap<String, Vec<NotificationEntry>>>>,
    error_listeners: Arc<RwLock<Vec<ErrorEntry>>>,
    close_listeners: Arc<RwLock<Vec<CloseEntry>>>,
    next_listener_id: AtomicU64,
    disposers: Mutex<Vec<Subscription>>,
}

impl McpClient {
    /// Create a client. Call [`Self::connect`] to start a transport.
    pub fn new(options: McpClientOptions) -> Self {
        let inner = Arc::new(ClientInner {
            options,
            state: Mutex::new(ClientState::Idle),
            transport: RwLock::new(None),
            next_request_id: AtomicI64::new(1),
            server_info: RwLock::new(None),
            server_capabilities: RwLock::new(None),
            instructions: RwLock::new(None),
            protocol_version: RwLock::new(None),
            pending: Mutex::new(HashMap::new()),
            progress_requests: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            request_handlers: Arc::new(RwLock::new(HashMap::new())),
            notification_listeners: Arc::new(RwLock::new(HashMap::new())),
            error_listeners: Arc::new(RwLock::new(Vec::new())),
            close_listeners: Arc::new(RwLock::new(Vec::new())),
            next_listener_id: AtomicU64::new(1),
            disposers: Mutex::new(Vec::new()),
        });
        let client = Self { inner };
        client.install_builtin_handlers();
        client
    }

    fn install_builtin_handlers(&self) {
        self.set_request_handler(
            "ping",
            Arc::new(|_params, _cancel| Box::pin(async { Ok(Value::Object(Map::new())) })),
        );
        let roots = self.inner.options.roots.as_ref().map(|_| ());
        if roots.is_some() {
            let inner = Arc::clone(&self.inner);
            self.set_request_handler(
                "roots/list",
                Arc::new(move |_params, _cancel| {
                    let inner = Arc::clone(&inner);
                    Box::pin(async move {
                        let roots = match &inner.options.roots {
                            Some(RootsProvider::Static(roots)) => roots.clone(),
                            Some(RootsProvider::Dynamic(provider)) => provider().await,
                            None => Vec::new(),
                        };
                        let mut object = Map::new();
                        object.insert("roots".to_string(), serde_json::to_value(roots)?);
                        Ok(Value::Object(object))
                    })
                }),
            );
        }
    }

    /// Current connection state.
    pub fn connection_state(&self) -> ClientState {
        *lock(&self.inner.state)
    }

    /// Server implementation info from `initialize`.
    pub fn server_info(&self) -> Option<Implementation> {
        read(&self.inner.server_info).clone()
    }

    /// Server capabilities from `initialize`.
    pub fn server_capabilities(&self) -> Option<ServerCapabilities> {
        read(&self.inner.server_capabilities).clone()
    }

    /// Server instructions from `initialize`.
    pub fn instructions(&self) -> Option<String> {
        read(&self.inner.instructions).clone()
    }

    /// Negotiated protocol version.
    pub fn protocol_version(&self) -> Option<String> {
        read(&self.inner.protocol_version).clone()
    }

    fn set_state(&self, state: ClientState) {
        *lock(&self.inner.state) = state;
    }

    fn transport(&self) -> Option<Arc<dyn McpTransport>> {
        read(&self.inner.transport).clone()
    }

    /// Connect a transport and run the MCP `initialize` handshake.
    pub async fn connect(&self, transport: Arc<dyn McpTransport>) -> Result<InitializeResult> {
        if self.connection_state() != ClientState::Idle {
            return Err(McpError::Transport(format!(
                "Cannot connect MCP client in {} state",
                self.connection_state()
            )));
        }
        self.set_state(ClientState::Connecting);
        *write(&self.inner.transport) = Some(Arc::clone(&transport));

        let subscriptions = vec![
            transport.on_message(Arc::new({
                let client = self.clone();
                move |message| client.handle_message(message)
            })),
            // Transport errors are reported only. Pending requests fail when
            // the transport closes.
            transport.on_error(Arc::new({
                let client = self.clone();
                move |error| client.emit_error(error)
            })),
            transport.on_close(Arc::new({
                let client = self.clone();
                move || client.handle_transport_close()
            })),
        ];
        *lock(&self.inner.disposers) = subscriptions;

        let result = async {
            transport.start().await?;
            let mut capabilities = self.inner.options.capabilities.clone();
            if self.inner.options.roots.is_some() && capabilities.roots.is_none() {
                capabilities.roots = Some(RootsCapability::default());
            }
            let params = InitializeParams {
                protocol_version: self
                    .inner
                    .options
                    .protocol_version
                    .clone()
                    .unwrap_or_else(|| LATEST_PROTOCOL_VERSION.to_string()),
                capabilities,
                client_info: Implementation {
                    name: self.inner.options.name.clone(),
                    version: self.inner.options.version.clone(),
                    title: self.inner.options.title.clone(),
                },
            };
            let value = self
                .request_internal("initialize", Some(serde_json::to_value(params)?), McpRequestOptions::default(), true)
                .await?;
            let result = validate_initialize_result(value)?;
            if !SUPPORTED_PROTOCOL_VERSIONS.contains(&result.protocol_version.as_str()) {
                return Err(McpError::InvalidResponse(format!(
                    "MCP server selected unsupported protocol version {}",
                    result.protocol_version
                )));
            }
            *write(&self.inner.protocol_version) = Some(result.protocol_version.clone());
            *write(&self.inner.server_info) = Some(result.server_info.clone());
            *write(&self.inner.server_capabilities) = Some(result.capabilities.clone());
            *write(&self.inner.instructions) = result.instructions.clone();
            transport.set_protocol_version(&result.protocol_version);
            self.notify_internal("notifications/initialized", None, true).await?;
            self.set_state(ClientState::Connected);
            Ok(result)
        }
        .await;

        if result.is_err() {
            let _ = self.close().await;
        }
        result
    }

    /// Send a request and wait for its response.
    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        options: McpRequestOptions,
    ) -> Result<Value> {
        self.request_internal(method, params, options, false).await
    }

    /// Send a notification.
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        self.notify_internal(method, params, false).await
    }

    async fn request_internal(
        &self,
        method: &str,
        params: Option<Value>,
        options: McpRequestOptions,
        allow_connecting: bool,
    ) -> Result<Value> {
        let transport = self.require_transport(allow_connecting)?;
        if options
            .cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(McpError::Aborted);
        }
        let id = JsonRpcId::Number(self.inner.next_request_id.fetch_add(1, Ordering::SeqCst));
        let progress_token = options.on_progress.as_ref().map(|_| id.clone());
        let request_params = with_progress_token(params, progress_token.as_ref());

        let timeout = options
            .timeout
            .or(self.inner.options.request_timeout)
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT);
        let (sender, mut receiver) = oneshot::channel();
        let timeout_reset = Arc::new(Notify::new());
        let send_cancel = CancellationToken::new();
        {
            let mut pending = lock(&self.inner.pending);
            pending.insert(
                id.clone(),
                PendingRequest {
                    sender,
                    cancellable: method != "initialize",
                    progress_token: progress_token.clone(),
                    on_progress: options.on_progress.clone(),
                    timeout_reset: Arc::clone(&timeout_reset),
                    send_cancel: send_cancel.clone(),
                },
            );
        }
        if let Some(token) = &progress_token {
            lock(&self.inner.progress_requests).insert(token.clone(), id.clone());
        }

        let message = JsonRpcMessage::request(id.clone(), method, request_params);
        if let Err(error) = transport
            .send(
                message,
                McpTransportSendOptions {
                    cancel: Some(send_cancel),
                },
            )
            .await
        {
            self.fail_pending(&id, error);
        }

        let mut deadline = Instant::now() + timeout;
        let cancel = options.cancel.clone();
        loop {
            let cancel_future = async {
                match &cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                biased;
                value = &mut receiver => {
                    return value.unwrap_or_else(|_| {
                        Err(McpError::ConnectionClosed("MCP client closed".to_string()))
                    });
                }
                () = cancel_future => {
                    self.cancel_pending(&id, McpError::Aborted, true, Some("Aborted"));
                    return Err(McpError::Aborted);
                }
                () = sleep_until(deadline) => {
                    let cancellable = {
                        let pending = lock(&self.inner.pending);
                        pending.get(&id).is_some_and(|entry| entry.cancellable)
                    };
                    self.cancel_pending(
                        &id,
                        McpError::Timeout(timeout),
                        cancellable,
                        Some("Request timed out"),
                    );
                    return Err(McpError::Timeout(timeout));
                }
                () = timeout_reset.notified() => {
                    deadline = Instant::now() + timeout;
                }
            }
        }
    }

    async fn notify_internal(
        &self,
        method: &str,
        params: Option<Value>,
        allow_connecting: bool,
    ) -> Result<()> {
        let transport = self.require_transport(allow_connecting)?;
        transport
            .send(
                JsonRpcMessage::notification(method, params),
                McpTransportSendOptions::default(),
            )
            .await
    }

    fn require_transport(&self, allow_connecting: bool) -> Result<Arc<dyn McpTransport>> {
        let state = self.connection_state();
        if state == ClientState::Connected || (allow_connecting && state == ClientState::Connecting) {
            if let Some(transport) = self.transport() {
                return Ok(transport);
            }
        }
        Err(McpError::ConnectionClosed(format!("MCP client is {state}")))
    }

    /// Register a handler for a server-to-client request.
    pub fn set_request_handler(&self, method: &str, handler: RequestHandler) -> Subscription {
        write(&self.inner.request_handlers).insert(method.to_string(), Arc::clone(&handler));
        let handlers = Arc::clone(&self.inner.request_handlers);
        let method = method.to_string();
        Subscription::from_fn(move || {
            let mut handlers = write(&handlers);
            if handlers
                .get(&method)
                .is_some_and(|current| Arc::ptr_eq(current, &handler))
            {
                handlers.remove(&method);
            }
        })
    }

    /// Subscribe to a notification method.
    pub fn on_notification(&self, method: &str, listener: NotificationListener) -> Subscription {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        write(&self.inner.notification_listeners)
            .entry(method.to_string())
            .or_default()
            .push((id, listener));
        let listeners = Arc::clone(&self.inner.notification_listeners);
        let method = method.to_string();
        Subscription::from_fn(move || {
            let mut listeners = write(&listeners);
            if let Some(entries) = listeners.get_mut(&method) {
                entries.retain(|(listener_id, _)| *listener_id != id);
                if entries.is_empty() {
                    listeners.remove(&method);
                }
            }
        })
    }

    /// Subscribe to transport errors.
    pub fn on_error(&self, listener: ErrorListener) -> Subscription {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        write(&self.inner.error_listeners).push((id, listener));
        let listeners = Arc::clone(&self.inner.error_listeners);
        Subscription::from_fn(move || {
            write(&listeners).retain(|(listener_id, _)| *listener_id != id);
        })
    }

    /// Subscribe to the close event.
    pub fn on_close(&self, listener: CloseListener) -> Subscription {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        write(&self.inner.close_listeners).push((id, listener));
        let listeners = Arc::clone(&self.inner.close_listeners);
        Subscription::from_fn(move || {
            write(&listeners).retain(|(listener_id, _)| *listener_id != id);
        })
    }

    /// Ping the server.
    pub async fn ping(&self, options: McpRequestOptions) -> Result<()> {
        self.request("ping", None, options).await.map(|_| ())
    }

    /// List every tool, following `nextCursor`.
    pub async fn list_tools(&self, options: McpRequestOptions) -> Result<Vec<Tool>> {
        let items = self
            .list_all("tools/list", "tools", options, |item| {
                item.get("name").is_some_and(Value::is_string)
                    && item.get("inputSchema").is_some_and(Value::is_object)
            })
            .await?;
        items
            .into_iter()
            .map(|item| serde_json::from_value(item).map_err(|error| McpError::InvalidResponse(error.to_string())))
            .collect()
    }

    /// List every resource, following `nextCursor`.
    pub async fn list_resources(&self, options: McpRequestOptions) -> Result<Vec<Resource>> {
        let items = self
            .list_all("resources/list", "resources", options, |item| {
                item.get("uri").is_some_and(Value::is_string)
                    && is_absent_or_string(item.get("name"))
            })
            .await?;
        items
            .into_iter()
            .map(|mut item| {
                if item.get("name").is_none() {
                    if let Some(uri) = item.get("uri").cloned() {
                        if let Some(object) = item.as_object_mut() {
                            object.insert("name".to_string(), uri);
                        }
                    }
                }
                serde_json::from_value(item).map_err(|error| McpError::InvalidResponse(error.to_string()))
            })
            .collect()
    }

    /// One page of resources starting at `cursor`.
    pub async fn list_resources_page(
        &self,
        cursor: Option<String>,
        options: McpRequestOptions,
    ) -> Result<ListResourcesResult> {
        let items = self
            .list_page("resources/list", "resources", cursor, options, |item| {
                item.get("uri").is_some_and(Value::is_string)
                    && is_absent_or_string(item.get("name"))
            })
            .await?;
        Ok(ListResourcesResult {
            resources: items
                .items
                .into_iter()
                .map(|mut item| {
                    if item.get("name").is_none() {
                        if let Some(uri) = item.get("uri").cloned() {
                            if let Some(object) = item.as_object_mut() {
                                object.insert("name".to_string(), uri);
                            }
                        }
                    }
                    serde_json::from_value(item)
                        .map_err(|error| McpError::InvalidResponse(error.to_string()))
                })
                .collect::<Result<Vec<_>>>()?,
            next_cursor: items.next_cursor,
            meta: None,
        })
    }

    /// List every resource template, following `nextCursor`.
    pub async fn list_resource_templates(
        &self,
        options: McpRequestOptions,
    ) -> Result<Vec<ResourceTemplate>> {
        let items = self
            .list_all("resources/templates/list", "resourceTemplates", options, |item| {
                item.get("uriTemplate").is_some_and(Value::is_string)
                    && is_absent_or_string(item.get("name"))
            })
            .await?;
        items
            .into_iter()
            .map(|mut item| {
                if item.get("name").is_none() {
                    if let Some(uri) = item.get("uriTemplate").cloned() {
                        if let Some(object) = item.as_object_mut() {
                            object.insert("name".to_string(), uri);
                        }
                    }
                }
                serde_json::from_value(item).map_err(|error| McpError::InvalidResponse(error.to_string()))
            })
            .collect()
    }

    /// One page of resource templates starting at `cursor`.
    pub async fn list_resource_templates_page(
        &self,
        cursor: Option<String>,
        options: McpRequestOptions,
    ) -> Result<ListResourceTemplatesResult> {
        let items = self
            .list_page("resources/templates/list", "resourceTemplates", cursor, options, |item| {
                item.get("uriTemplate").is_some_and(Value::is_string)
                    && is_absent_or_string(item.get("name"))
            })
            .await?;
        Ok(ListResourceTemplatesResult {
            resource_templates: items
                .items
                .into_iter()
                .map(|mut item| {
                    if item.get("name").is_none() {
                        if let Some(uri) = item.get("uriTemplate").cloned() {
                            if let Some(object) = item.as_object_mut() {
                                object.insert("name".to_string(), uri);
                            }
                        }
                    }
                    serde_json::from_value(item)
                        .map_err(|error| McpError::InvalidResponse(error.to_string()))
                })
                .collect::<Result<Vec<_>>>()?,
            next_cursor: items.next_cursor,
            meta: None,
        })
    }

    /// Read a resource.
    pub async fn read_resource(
        &self,
        uri: &str,
        options: McpRequestOptions,
    ) -> Result<crate::protocol::types::ReadResourceResult> {
        let mut params = Map::new();
        params.insert("uri".to_string(), Value::String(uri.to_string()));
        let value = self
            .request("resources/read", Some(Value::Object(params)), options)
            .await?;
        validate_read_resource_result(value)
    }

    /// Call a tool.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
        options: McpRequestOptions,
    ) -> Result<CallToolResult> {
        let mut params = Map::new();
        params.insert("name".to_string(), Value::String(name.to_string()));
        if let Some(arguments) = arguments {
            params.insert("arguments".to_string(), Value::Object(arguments));
        }
        let value = self
            .request("tools/call", Some(Value::Object(params)), options)
            .await?;
        validate_call_tool_result(value)
    }

    async fn list_page(
        &self,
        method: &str,
        key: &str,
        cursor: Option<String>,
        options: McpRequestOptions,
        is_item: impl Fn(&Map<String, Value>) -> bool,
    ) -> Result<ListPage> {
        let params = cursor.map(|cursor| {
            let mut params = Map::new();
            params.insert("cursor".to_string(), Value::String(cursor));
            Value::Object(params)
        });
        let value = self.request(method, params, options).await?;
        validate_list_page(method, key, value, &is_item)
    }

    async fn list_all(
        &self,
        method: &str,
        key: &str,
        options: McpRequestOptions,
        is_item: impl Fn(&Map<String, Value>) -> bool,
    ) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut cursors = HashSet::new();
        let mut cursor = None;
        for _ in 0..MAX_LIST_PAGES {
            let page = self
                .list_page(method, key, cursor, options.clone(), &is_item)
                .await?;
            items.extend(page.items);
            let Some(next) = page.next_cursor else {
                return Ok(items);
            };
            if !cursors.insert(next.clone()) {
                return Err(McpError::InvalidResponse(format!(
                    "MCP {method} returned duplicate cursor: {next}"
                )));
            }
            cursor = Some(next);
        }
        Err(McpError::InvalidResponse(format!(
            "MCP {method} exceeded {MAX_LIST_PAGES} pages"
        )))
    }

    /// Close the client and its transport.
    pub async fn close(&self) -> Result<()> {
        let transport = write(&self.inner.transport).take();
        let disposers = std::mem::take(&mut *lock(&self.inner.disposers));
        drop(disposers);
        self.mark_closed(McpError::ConnectionClosed("MCP connection closed".to_string()));
        if let Some(transport) = transport {
            transport.close().await?;
        }
        Ok(())
    }

    fn handle_message(&self, message: JsonRpcMessage) {
        match message {
            JsonRpcMessage::Response(response) => self.handle_response(response),
            JsonRpcMessage::Request(request) => {
                let client = self.clone();
                tokio::spawn(async move {
                    client.handle_request(request).await;
                });
            }
            JsonRpcMessage::Notification(notification) => {
                self.handle_notification(&notification.method, notification.params);
            }
        }
    }

    fn handle_response(&self, response: JsonRpcResponse) {
        let id = response.id().clone();
        let entry = {
            let mut pending = lock(&self.inner.pending);
            pending.remove(&id)
        };
        let Some(entry) = entry else {
            self.emit_error(&McpError::InvalidResponse(format!(
                "Received response for unknown MCP request {id}"
            )));
            return;
        };
        self.finish_pending(&entry);
        let value = match response {
            JsonRpcResponse::Success { result, .. } => Ok(result),
            JsonRpcResponse::Error { error, .. } => Err(McpError::JsonRpc {
                code: error.code,
                message: error.message,
                data: error.data,
            }),
        };
        let _ = entry.sender.send(value);
    }

    async fn handle_request(&self, request: crate::protocol::jsonrpc::JsonRpcRequest) {
        let Some(transport) = self.transport() else {
            return;
        };
        let handler = read(&self.inner.request_handlers)
            .get(&request.method)
            .cloned();
        let Some(handler) = handler else {
            let error = JsonRpcErrorObject {
                code: JSON_RPC_ERROR_CODES.method_not_found,
                message: format!("Method not found: {}", request.method),
                data: None,
            };
            let _ = transport
                .send(
                    JsonRpcMessage::error(request.id, error),
                    McpTransportSendOptions::default(),
                )
                .await;
            return;
        };
        let cancel = CancellationToken::new();
        lock(&self.inner.incoming).insert(request.id.clone(), cancel.clone());
        let result = handler(request.params, cancel).await;
        lock(&self.inner.incoming).remove(&request.id);
        let response = match result {
            Ok(value) => JsonRpcMessage::success(
                request.id,
                if value.is_null() { Value::Object(Map::new()) } else { value },
            ),
            Err(McpError::JsonRpc {
                code, message, data, ..
            }) => JsonRpcMessage::error(
                request.id,
                JsonRpcErrorObject {
                    code,
                    message,
                    data,
                },
            ),
            Err(error) => JsonRpcMessage::error(
                request.id,
                JsonRpcErrorObject {
                    code: JSON_RPC_ERROR_CODES.internal_error,
                    message: error.to_string(),
                    data: None,
                },
            ),
        };
        if let Err(error) = transport
            .send(response, McpTransportSendOptions::default())
            .await
        {
            self.emit_error(&error);
        }
    }

    fn handle_notification(&self, method: &str, params: Option<Value>) {
        if method == "notifications/progress" {
            self.handle_progress(params.as_ref());
        } else if method == "notifications/cancelled" {
            self.handle_cancelled(params.as_ref());
        }
        let listeners = read(&self.inner.notification_listeners)
            .get(method)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(_, listener)| Arc::clone(listener))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for listener in listeners {
            listener(params.clone());
        }
    }

    fn handle_progress(&self, params: Option<&Value>) {
        let Some(value) = params else { return };
        let Ok(notification) = serde_json::from_value::<ProgressNotification>(value.clone()) else {
            return;
        };
        let request_id = lock(&self.inner.progress_requests).get(&notification.progress_token).cloned();
        let Some(request_id) = request_id else {
            return;
        };
        let (on_progress, reset) = {
            let pending = lock(&self.inner.pending);
            match pending.get(&request_id) {
                Some(entry) => (entry.on_progress.clone(), Arc::clone(&entry.timeout_reset)),
                None => return,
            }
        };
        reset.notify_one();
        if let Some(on_progress) = on_progress {
            on_progress(notification);
        }
    }

    fn handle_cancelled(&self, params: Option<&Value>) {
        let Some(value) = params else { return };
        let Ok(cancelled) = serde_json::from_value::<CancelledNotification>(value.clone()) else {
            return;
        };
        if let Some(token) = lock(&self.inner.incoming).get(&cancelled.request_id) {
            token.cancel();
        }
    }

    fn fail_pending(&self, id: &JsonRpcId, error: McpError) {
        if let Some(entry) = {
            let mut pending = lock(&self.inner.pending);
            pending.remove(id)
        } {
            self.finish_pending(&entry);
            let _ = entry.sender.send(Err(error));
        }
    }

    fn cancel_pending(&self, id: &JsonRpcId, error: McpError, notify_server: bool, reason: Option<&str>) {
        let Some(entry) = ({
            let mut pending = lock(&self.inner.pending);
            pending.remove(id)
        }) else {
            return;
        };
        self.finish_pending(&entry);
        let _ = entry.sender.send(Err(error));
        if !notify_server {
            return;
        }
        if let Some(transport) = self.transport() {
            let mut params = Map::new();
            params.insert("requestId".to_string(), serde_json::to_value(id).unwrap_or(Value::Null));
            if let Some(reason) = reason {
                params.insert("reason".to_string(), Value::String(reason.to_string()));
            }
            tokio::spawn(async move {
                let _ = transport
                    .send(
                        JsonRpcMessage::notification("notifications/cancelled", Some(Value::Object(params))),
                        McpTransportSendOptions::default(),
                    )
                    .await;
            });
        }
    }

    fn finish_pending(&self, entry: &PendingRequest) {
        entry.send_cancel.cancel();
        if let Some(token) = &entry.progress_token {
            lock(&self.inner.progress_requests).remove(token);
        }
    }

    fn handle_transport_close(&self) {
        self.mark_closed(McpError::ConnectionClosed("MCP connection closed".to_string()));
    }

    fn mark_closed(&self, error: McpError) {
        let was_closed = {
            let mut state = lock(&self.inner.state);
            let was_closed = *state == ClientState::Closed;
            *state = ClientState::Closed;
            was_closed
        };
        let pending = {
            let mut pending = lock(&self.inner.pending);
            pending.drain().map(|(_, entry)| entry).collect::<Vec<_>>()
        };
        for entry in pending {
            self.finish_pending(&entry);
            let _ = entry.sender.send(Err(McpError::ConnectionClosed(error.to_string())));
        }
        let incoming = {
            let mut incoming = lock(&self.inner.incoming);
            incoming.drain().map(|(_, token)| token).collect::<Vec<_>>()
        };
        for token in incoming {
            token.cancel();
        }
        if was_closed {
            return;
        }
        let listeners = read(&self.inner.close_listeners)
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect::<Vec<_>>();
        for listener in listeners {
            listener();
        }
    }

    fn emit_error(&self, error: &McpError) {
        let listeners = read(&self.inner.error_listeners)
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect::<Vec<_>>();
        for listener in listeners {
            listener(error);
        }
    }
}

fn with_progress_token(params: Option<Value>, token: Option<&JsonRpcId>) -> Option<Value> {
    let Some(token) = token else {
        return params;
    };
    let mut object = match params {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    let mut meta = match object.remove("_meta") {
        Some(Value::Object(meta)) => meta,
        _ => Map::new(),
    };
    meta.insert(
        "progressToken".to_string(),
        serde_json::to_value(token).unwrap_or(Value::Null),
    );
    object.insert("_meta".to_string(), Value::Object(meta));
    Some(Value::Object(object))
}

fn validate_initialize_result(value: Value) -> Result<InitializeResult> {
    let Some(object) = value.as_object() else {
        return Err(McpError::invalid_request("Invalid MCP initialize result"));
    };
    let valid = object.get("protocolVersion").is_some_and(Value::is_string)
        && object.get("capabilities").is_some_and(Value::is_object)
        && object
            .get("serverInfo")
            .and_then(Value::as_object)
            .is_some_and(|info| {
                info.get("name").is_some_and(Value::is_string)
                    && info.get("version").is_some_and(Value::is_string)
            })
        && is_absent_or_string(object.get("instructions"));
    if !valid {
        return Err(McpError::invalid_request("Invalid MCP initialize result"));
    }
    serde_json::from_value(value).map_err(|error| McpError::invalid_request(error.to_string()))
}

fn is_absent_or_string(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(value) => value.is_string(),
    }
}

struct ListPage {
    items: Vec<Value>,
    next_cursor: Option<String>,
}

fn validate_list_page(
    method: &str,
    key: &str,
    value: Value,
    is_item: &impl Fn(&Map<String, Value>) -> bool,
) -> Result<ListPage> {
    let Some(object) = value.as_object() else {
        return Err(McpError::invalid_request(format!("Invalid MCP {method} result")));
    };
    let Some(items) = object.get(key).and_then(Value::as_array) else {
        return Err(McpError::invalid_request(format!("Invalid MCP {method} result")));
    };
    for item in items {
        let Some(item) = item.as_object() else {
            return Err(McpError::invalid_request(format!(
                "Invalid entry in MCP {method} result"
            )));
        };
        if !is_item(item) {
            return Err(McpError::invalid_request(format!(
                "Invalid entry in MCP {method} result"
            )));
        }
    }
    let next_cursor = match object.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) if cursor.is_empty() => None,
        Some(Value::String(cursor)) => Some(cursor.clone()),
        Some(_) => {
            return Err(McpError::invalid_request(format!(
                "Invalid MCP {method} cursor"
            )))
        }
    };
    Ok(ListPage {
        items: items.clone(),
        next_cursor,
    })
}

fn validate_read_resource_result(value: Value) -> Result<crate::protocol::types::ReadResourceResult> {
    let Some(object) = value.as_object() else {
        return Err(McpError::invalid_request("Invalid MCP resources/read result"));
    };
    let Some(contents) = object.get("contents").and_then(Value::as_array) else {
        return Err(McpError::invalid_request("Invalid MCP resources/read result"));
    };
    for item in contents {
        let Some(item) = item.as_object() else {
            return Err(McpError::invalid_request(
                "Invalid contents in MCP resources/read result",
            ));
        };
        let valid = item.get("uri").is_some_and(Value::is_string)
            && (item.get("text").is_some_and(Value::is_string)
                || item.get("blob").is_some_and(Value::is_string));
        if !valid {
            return Err(McpError::invalid_request(
                "Invalid contents in MCP resources/read result",
            ));
        }
    }
    serde_json::from_value(value).map_err(|error| McpError::invalid_request(error.to_string()))
}

fn validate_call_tool_result(mut value: Value) -> Result<CallToolResult> {
    let Some(object) = value.as_object_mut() else {
        return Err(McpError::invalid_request("Invalid MCP tools/call result"));
    };
    if let Some(content) = object.get("content") {
        if !content.is_array() {
            return Err(McpError::invalid_request("Invalid MCP tools/call result"));
        }
    } else {
        object.insert("content".to_string(), Value::Array(Vec::new()));
    }
    serde_json::from_value(value).map_err(|error| McpError::invalid_request(error.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn progress_token_is_merged_into_meta() {
        let token = JsonRpcId::Number(3);
        let params = Some(serde_json::json!({"cursor": "a", "_meta": {"x": 1}}));
        let merged = with_progress_token(params, Some(&token)).unwrap();
        assert_eq!(merged["_meta"]["progressToken"], 3);
        assert_eq!(merged["_meta"]["x"], 1);
        assert_eq!(merged["cursor"], "a");
    }

    #[test]
    fn list_page_treats_null_and_empty_cursor_as_end() {
        let page = validate_list_page(
            "tools/list",
            "tools",
            serde_json::json!({"tools": [], "nextCursor": null}),
            &|_| true,
        )
        .unwrap();
        assert!(page.next_cursor.is_none());
        let page = validate_list_page(
            "tools/list",
            "tools",
            serde_json::json!({"tools": [], "nextCursor": ""}),
            &|_| true,
        )
        .unwrap();
        assert!(page.next_cursor.is_none());
    }
}
