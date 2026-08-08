//! Extension action bus — lets JS extension actions reach the `AgentSession`.
//!
//! The V8 runtime thread is separate from the thread that owns the
//! `AgentSession`, and the session is driven synchronously by its owner
//! (CLI / RPC / ACP mode). This module provides the bridge:
//!
//! - **`ExtensionStateView`** — a shared snapshot of session state, refreshed
//!   by the session at drain points (turn boundaries). Read-actions
//!   (`getActiveTools`, `getAllTools`, `getSessionName`, `getThinkingLevel`,
//!   `getCommands`) read this snapshot synchronously.
//! - **`ExtensionAction`** — write-commands queued by the extension and
//!   applied by the session at the next drain point (`sendMessage`,
//!   `sendUserMessage`, `appendEntry`, `setSessionName`, `setLabel`,
//!   `setActiveTools`, `setThinkingLevel`, `setModel`).
//!
//! This mirrors TS's `ExtensionRunner.bindCore()` closures: the JS side calls
//! an op, the op invokes a closure that either reads the snapshot or enqueues
//! a command; the session applies commands when it is driven next.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

/// Shared snapshot of session state that extension read-actions see.
/// Refreshed by `AgentSession::refresh_extension_state()` at drain points.
#[derive(Default)]
pub struct ExtensionStateView {
    pub session_name: Option<String>,
    pub active_tools: Vec<String>,
    pub all_tools: Vec<serde_json::Value>,
    pub thinking_level: String,
    pub commands: Vec<serde_json::Value>,
    pub model_id: Option<String>,
}

/// Write-actions queued by JS extensions and applied by the session at the
/// next drain point (turn boundary).
#[derive(Debug)]
pub enum ExtensionAction {
    SendMessage {
        custom_type: String,
        content: String,
        options_json: Option<String>,
    },
    SendUserMessage {
        content: String,
        options_json: Option<String>,
    },
    AppendEntry {
        custom_type: String,
        data_json: Option<String>,
    },
    SetSessionName(String),
    SetLabel {
        entry_id: String,
        label: Option<String>,
    },
    SetActiveTools(Vec<String>),
    SetThinkingLevel(String),
    SetModel(String),
}

/// Cloneable handle held by the extension-runtime closures: read-actions read
/// the shared state view; write-actions enqueue onto the command channel.
#[derive(Clone)]
pub struct ExtensionActionSender {
    tx: mpsc::UnboundedSender<ExtensionAction>,
    state: Arc<Mutex<ExtensionStateView>>,
}

impl ExtensionActionSender {
    /// Create the channel + shared state view. The caller passes the receiver
    /// and the state-view `Arc` into the `AgentSession` (via config), and
    /// keeps this sender for the bind_core closures.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<ExtensionAction>, Arc<Mutex<ExtensionStateView>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(ExtensionStateView::default()));
        (
            Self {
                tx,
                state: state.clone(),
            },
            rx,
            state,
        )
    }

    /// Enqueue a write-action for the session to apply at the next drain point.
    pub fn send(&self, action: ExtensionAction) {
        let _ = self.tx.send(action);
    }

    /// Read the shared state snapshot.
    pub fn state(&self) -> Arc<Mutex<ExtensionStateView>> {
        self.state.clone()
    }
}
