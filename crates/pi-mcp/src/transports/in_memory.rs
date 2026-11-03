//! In-memory transport pair, used by client and OAuth tests.
//!
//! Port of `packages/mcp/src/transports/in-memory.ts`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::error::{McpError, Result};
use crate::protocol::jsonrpc::JsonRpcMessage;
use crate::transport::{
    CloseListener, ErrorListener, McpTransport, McpTransportSendOptions, MessageListener,
    Subscription, TransportEvents,
};

/// One side of an in-memory transport pair.
pub struct InMemoryTransport {
    events: TransportEvents,
    peer: RwLock<Option<Arc<InMemoryTransport>>>,
    started: AtomicBool,
    closed: AtomicBool,
}

impl InMemoryTransport {
    /// Create an unconnected endpoint.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            events: TransportEvents::new(),
            peer: RwLock::new(None),
            started: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    /// Connect this endpoint to its peer. Only one peer is allowed.
    pub fn connect_peer(&self, peer: Arc<InMemoryTransport>) -> Result<()> {
        let mut slot = self
            .peer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return Err(McpError::Transport(
                "In-memory MCP transport already has a peer".to_string(),
            ));
        }
        *slot = Some(peer);
        Ok(())
    }

    /// Emit a transport error (test hook).
    pub fn emit_error(&self, error: McpError) {
        self.events.emit_error(&error);
    }

    fn deliver(&self, message: JsonRpcMessage) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        self.events.emit_message(message);
    }
}

#[async_trait]
impl McpTransport for InMemoryTransport {
    async fn start(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
        }
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, message: JsonRpcMessage, options: McpTransportSendOptions) -> Result<()> {
        if !self.started.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
        }
        if options.is_cancelled() {
            return Err(McpError::Aborted);
        }
        let peer = self
            .peer
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| McpError::ConnectionClosed("In-memory MCP peer is not connected".to_string()))?;
        if !peer.started.load(Ordering::SeqCst) || peer.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed(
                "In-memory MCP peer is not connected".to_string(),
            ));
        }
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            peer.deliver(message);
        });
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.events.emit_close();
        let peer = self
            .peer
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(peer) = peer {
            tokio::spawn(async move {
                let _ = peer.close().await;
            });
        }
        Ok(())
    }

    fn on_message(&self, listener: MessageListener) -> Subscription {
        self.events.on_message(listener)
    }

    fn on_error(&self, listener: ErrorListener) -> Subscription {
        self.events.on_error(listener)
    }

    fn on_close(&self, listener: CloseListener) -> Subscription {
        self.events.on_close(listener)
    }
}

/// Create a connected pair of transports.
pub fn create_in_memory_transport_pair() -> (Arc<InMemoryTransport>, Arc<InMemoryTransport>) {
    let client = InMemoryTransport::new();
    let server = InMemoryTransport::new();
    // `connect_peer` only fails if a peer was already set, which cannot happen
    // for freshly created endpoints.
    let _ = client.connect_peer(Arc::clone(&server));
    let _ = server.connect_peer(Arc::clone(&client));
    (client, server)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[tokio::test]
    async fn messages_reach_the_peer() {
        let (client, server) = create_in_memory_transport_pair();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let _subscription = server.on_message(Arc::new(move |message| {
            let _ = sender.send(message);
        }));
        client.start().await.unwrap();
        server.start().await.unwrap();
        client
            .send(
                JsonRpcMessage::notification("hello", None),
                McpTransportSendOptions::default(),
            )
            .await
            .unwrap();
        let message = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.method(), Some("hello"));
    }
}
