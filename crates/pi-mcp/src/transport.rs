//! Transport abstraction shared by stdio, streamable HTTP, and in-memory.
//!
//! Port of `packages/mcp/src/transports/transport.ts`. Listeners are plain
//! callbacks; [`Subscription`] removes the listener on drop like the disposal
//! functions returned by the TypeScript API.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::protocol::jsonrpc::JsonRpcMessage;

/// Default maximum size of one JSON-RPC message (16 MiB).
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Callback invoked for every incoming JSON-RPC message.
pub type MessageListener = Arc<dyn Fn(JsonRpcMessage) + Send + Sync>;
/// Callback invoked for transport-level errors.
pub type ErrorListener = Arc<dyn Fn(&crate::error::McpError) + Send + Sync>;
/// Callback invoked once when the transport closes.
pub type CloseListener = Arc<dyn Fn() + Send + Sync>;

type MessageEntry = (u64, MessageListener);
type ErrorEntry = (u64, ErrorListener);
type CloseEntry = (u64, CloseListener);

/// Removes a listener when dropped.
pub struct Subscription {
    remove: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Subscription {
    /// A subscription that does nothing on drop.
    pub fn empty() -> Self {
        Self { remove: None }
    }

    /// Build a subscription from a removal callback.
    pub fn from_fn(remove: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            remove: Some(Arc::new(remove)),
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(remove) = self.remove.take() {
            remove();
        }
    }
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("active", &self.remove.is_some())
            .finish()
    }
}

/// Listener bookkeeping shared by transports.
///
/// `emit_close` fires at most once; the flag is per transport instance.
#[derive(Default)]
pub struct TransportEvents {
    message_listeners: Arc<RwLock<Vec<MessageEntry>>>,
    error_listeners: Arc<RwLock<Vec<ErrorEntry>>>,
    close_listeners: Arc<RwLock<Vec<CloseEntry>>>,
    next_id: AtomicU64,
    close_emitted: AtomicBool,
}

impl TransportEvents {
    /// Create an empty event set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to incoming messages.
    pub fn on_message(&self, listener: MessageListener) -> Subscription {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut listeners) = self.message_listeners.write() {
            listeners.push((id, listener));
        }
        let guard = Arc::clone(&self.message_listeners);
        Subscription {
            remove: Some(Arc::new(move || {
                if let Ok(mut listeners) = guard.write() {
                    listeners.retain(|(listener_id, _)| *listener_id != id);
                }
            })),
        }
    }

    /// Subscribe to transport errors.
    pub fn on_error(&self, listener: ErrorListener) -> Subscription {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut listeners) = self.error_listeners.write() {
            listeners.push((id, listener));
        }
        let guard = Arc::clone(&self.error_listeners);
        Subscription {
            remove: Some(Arc::new(move || {
                if let Ok(mut listeners) = guard.write() {
                    listeners.retain(|(listener_id, _)| *listener_id != id);
                }
            })),
        }
    }

    /// Subscribe to the close event.
    pub fn on_close(&self, listener: CloseListener) -> Subscription {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut listeners) = self.close_listeners.write() {
            listeners.push((id, listener));
        }
        let guard = Arc::clone(&self.close_listeners);
        Subscription {
            remove: Some(Arc::new(move || {
                if let Ok(mut listeners) = guard.write() {
                    listeners.retain(|(listener_id, _)| *listener_id != id);
                }
            })),
        }
    }

    /// Deliver a message to every current listener.
    pub fn emit_message(&self, message: JsonRpcMessage) {
        let listeners = self
            .message_listeners
            .read()
            .map(|listeners| listeners.iter().map(|(_, listener)| Arc::clone(listener)).collect::<Vec<_>>())
            .unwrap_or_default();
        for listener in listeners {
            listener(message.clone());
        }
    }

    /// Deliver an error to every current listener.
    pub fn emit_error(&self, error: &crate::error::McpError) {
        let listeners = self
            .error_listeners
            .read()
            .map(|listeners| listeners.iter().map(|(_, listener)| Arc::clone(listener)).collect::<Vec<_>>())
            .unwrap_or_default();
        for listener in listeners {
            listener(error);
        }
    }

    /// Fire the close event at most once.
    pub fn emit_close(&self) {
        if self.close_emitted.swap(true, Ordering::SeqCst) {
            return;
        }
        let listeners = self
            .close_listeners
            .read()
            .map(|listeners| listeners.iter().map(|(_, listener)| Arc::clone(listener)).collect::<Vec<_>>())
            .unwrap_or_default();
        for listener in listeners {
            listener();
        }
    }
}

/// Options for [`McpTransport::send`].
#[derive(Debug, Clone, Default)]
pub struct McpTransportSendOptions {
    /// Aborted once the sender no longer waits for a reply. A transport must
    /// not dispatch the message after this fires, for example when retrying
    /// after an authorization refresh.
    pub cancel: Option<CancellationToken>,
}

impl McpTransportSendOptions {
    /// Whether dispatch should be skipped.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancellationToken::is_cancelled)
    }
}

/// A bidirectional JSON-RPC transport.
///
/// Methods take `&self` (not `&mut self`) so the client can keep the
/// transport behind an `Arc` and share it with event listeners; transports
/// use interior mutability for their lifecycle state.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Start the transport (spawn a process, open a stream, ...).
    async fn start(&self) -> Result<()>;
    /// Send one message.
    async fn send(&self, message: JsonRpcMessage, options: McpTransportSendOptions) -> Result<()>;
    /// Close the transport.
    async fn close(&self) -> Result<()>;
    /// Subscribe to incoming messages.
    fn on_message(&self, listener: MessageListener) -> Subscription;
    /// Subscribe to transport errors.
    fn on_error(&self, listener: ErrorListener) -> Subscription;
    /// Subscribe to the close event.
    fn on_close(&self, listener: CloseListener) -> Subscription;
    /// Tell HTTP transports which protocol version to send.
    fn set_protocol_version(&self, _version: &str) {}
}
